//! `gsx-client` — Rust client SDK for the gsx-dag JSON-RPC query API.
//!
//! # Stability promise
//!
//! This crate follows semver with the following 0.x carve-outs:
//!
//! - **Within a minor version** (e.g. 0.2.0 → 0.2.5), method
//!   signatures, request/response types, and error variants are
//!   stable. Bug fixes only.
//! - **Between minor versions** (e.g. 0.2 → 0.3), the public API may
//!   gain new methods or fields; existing method signatures will not
//!   break, but new variants may appear on `#[non_exhaustive]` enums
//!   (notably `gsx_rpc::error::RpcError` and `gsx_execution::Intent`).
//! - **At 1.0**, signatures freeze under the standard semver guarantee
//!   and breaking changes require a major bump.
//!
//! Downstream code that matches on `Intent` or `RpcError` must include
//! a wildcard arm — those enums are marked `#[non_exhaustive]`
//! precisely so adding a new variant in a future protocol revision
//! (Phase G3/G4 governance ops, future application-level RPC errors)
//! is a non-breaking change for SDK consumers. `LeaderStatus` from
//! `gsx_consensus` is deliberately exhaustive (Direct/Skip/Undecided)
//! because it tracks the paper's canonical commit-rule outcomes; any
//! fourth state would be a paper-level amendment and a major bump.
//!
//! Wraps the JSON-RPC 2.0 methods exposed by `gsx-rpc` (bound into the
//! daemon by `crates/gsx-node/src/rpc_adapter.rs`). The current method
//! surface is read-only (Phase 2.1 MVP):
//!
//! - [`Client::get_epoch`]
//! - [`Client::get_authority_registry`]
//! - [`Client::get_validator_registry`]
//! - [`Client::get_stake`]
//!
//! The view types ([`EpochView`], [`AuthorityMemberView`],
//! [`ValidatorMemberView`]) are re-exported from `gsx-rpc` so a binary
//! crate that depends on both stays compatible.
//!
//! ## Quick start
//!
//! Point at the public hosted devnet:
//!
//! ```no_run
//! # async fn doc() -> Result<(), gsx_client::Error> {
//! let client = gsx_client::Client::new("https://rpc.devnet.gsx.globalsettlement.com");
//! let epoch = client.get_epoch().await?;
//! println!(
//!     "epoch={} rounds_per_epoch={} last_boundary_round={}",
//!     epoch.current, epoch.rounds_per_epoch, epoch.last_boundary_round
//! );
//! # Ok(())
//! # }
//! ```
//!
//! Or against a local 4-node devnet
//! (`./scripts/devnet-local.sh up`): `Client::new("http://127.0.0.1:9092")`.
//!
//! Construction returns `Client` directly (no `Result`) — TCP reachability
//! is deferred to the first method call, which surfaces transport errors
//! as [`Error::Transport`].

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod error;

use std::sync::atomic::{AtomicU64, Ordering};

pub use error::Error;
pub use gsx_crypto::mldsa::{self, PublicKey, SecretKey};
pub use gsx_execution::Intent;
pub use gsx_rpc::context::{
    AuthorityMemberView, BalanceView, BlockView, EpochView, IntentView, TransactionView,
    ValidatorMemberView,
};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};

/// JSON-RPC client targeting a single gsx-dag node's RPC endpoint.
///
/// Cheap to clone (internally just an `Arc` to the reqwest client plus
/// the base URL). The auto-incrementing JSON-RPC `id` field is shared
/// across clones so concurrent calls don't collide.
pub struct Client {
    http: reqwest::Client,
    base_url: String,
    next_id: std::sync::Arc<AtomicU64>,
}

impl Client {
    /// Construct a client targeting `base_url` (e.g. `"http://localhost:9092"`).
    ///
    /// No TCP connection is opened until the first RPC method is called.
    /// `base_url` should NOT include a trailing path component —
    /// the JSON-RPC endpoint is the root `/` of the server.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self::with_http_client(reqwest::Client::new(), base_url)
    }

    /// Construct a client with a caller-supplied `reqwest::Client`.
    /// Useful for tests, or to share a connection pool / inject middleware.
    pub fn with_http_client(http: reqwest::Client, base_url: impl Into<String>) -> Self {
        Self {
            http,
            base_url: base_url.into(),
            next_id: std::sync::Arc::new(AtomicU64::new(1)),
        }
    }

    /// Current epoch snapshot.
    pub async fn get_epoch(&self) -> Result<EpochView, Error> {
        self.call("gsx_getEpoch", Value::Null).await
    }

    /// Ordered list of seated Authority Ring members.
    pub async fn get_authority_registry(&self) -> Result<Vec<AuthorityMemberView>, Error> {
        self.call("gsx_getAuthorityRegistry", Value::Null).await
    }

    /// Ordered list of seated Validator Ring members.
    pub async fn get_validator_registry(&self) -> Result<Vec<ValidatorMemberView>, Error> {
        self.call("gsx_getValidatorRegistry", Value::Null).await
    }

    /// Posted stake for a specific authority id. Returns `Ok(None)` for
    /// the application-level "not found" code (-32000) and `Err` for
    /// any other error class.
    pub async fn get_stake(&self, authority_id: u32) -> Result<Option<StakeEntry>, Error> {
        match self
            .call::<StakeEntry>("gsx_getStake", json!({ "authority_id": authority_id }))
            .await
        {
            Ok(entry) => Ok(Some(entry)),
            Err(Error::Rpc { code: -32000, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Substrate balance for `address`. Always returns `Ok(BalanceView)` —
    /// unknown addresses surface as `balance == "0"` (the substrate
    /// doesn't distinguish absent from explicit-zero). Use
    /// [`Client::get_stake`] if you need the NotFound translation pattern.
    pub async fn get_balance(&self, address: [u8; 20]) -> Result<BalanceView, Error> {
        let hex_addr = format!("0x{}", hex::encode(address));
        self.call::<BalanceView>("gsx_getBalance", json!({ "address": hex_addr }))
            .await
    }

    /// Committed block at `round`. Returns `Ok(None)` for the
    /// application-level NotFound code (no block committed at that
    /// round, e.g. because the leader was skipped or the round is in
    /// the future). Other errors propagate.
    pub async fn get_block(&self, round: u64) -> Result<Option<BlockView>, Error> {
        match self
            .call::<BlockView>("gsx_getBlock", json!({ "round": round }))
            .await
        {
            Ok(v) => Ok(Some(v)),
            Err(Error::Rpc { code: -32000, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Committed transaction by intent hash. Returns `Ok(None)` for the
    /// application-level NotFound code; other errors propagate.
    pub async fn get_transaction(
        &self,
        tx_hash: [u8; 32],
    ) -> Result<Option<TransactionView>, Error> {
        let hex_h = format!("0x{}", hex::encode(tx_hash));
        match self
            .call::<TransactionView>("gsx_getTransaction", json!({ "tx_hash": hex_h }))
            .await
        {
            Ok(v) => Ok(Some(v)),
            Err(Error::Rpc { code: -32000, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Submit a signed intent for inclusion in the next block.
    ///
    /// **Low-level** — the caller is responsible for:
    ///
    /// 1. bincode-serializing the typed `gsx_execution::Intent` into
    ///    `intent_bincode`. This SDK doesn't depend on `gsx-execution`
    ///    yet so the encoding stays on the caller side; a typed helper
    ///    `submit_signed(Intent, &SecretKey, &str)` lands in a follow-up.
    /// 2. Computing the signing digest:
    ///    `blake3(b"GSX_INTENT_V1" || network_id_bytes || intent_bincode)`
    ///    and signing it with ML-DSA-65.
    /// 3. Computing `blake3(public_key_bytes)` for `signer_pubkey_hash`.
    ///
    /// `signer_pubkey` is required for **open signers** — callers not
    /// seated in the Authority or Validator Ring. Ring members may pass
    /// `None`; the server resolves their public key from the registry
    /// via `signer_pubkey_hash`. Open signers submitting user-tier
    /// intents (Transfer, Delegate, etc.) must provide their raw
    /// ML-DSA-65 public key (1,952 bytes) so the server can verify
    /// the signature.
    ///
    /// Returns the daemon's computed intent hash on success (same as
    /// what will appear in `gsx_getTransaction` lookups).
    pub async fn submit_intent_raw(
        &self,
        intent_bincode: &[u8],
        signature: &[u8],
        signer_pubkey_hash: [u8; 32],
        signer_pubkey: Option<&[u8]>,
    ) -> Result<[u8; 32], Error> {
        #[derive(serde::Deserialize)]
        struct Ack {
            tx_hash: String,
        }
        let mut params = json!({
            "intent": format!("0x{}", hex::encode(intent_bincode)),
            "signature": format!("0x{}", hex::encode(signature)),
            "signer_pubkey_hash": format!("0x{}", hex::encode(signer_pubkey_hash)),
        });
        if let Some(pk) = signer_pubkey {
            params["signer_pubkey"] = json!(format!("0x{}", hex::encode(pk)));
        }
        let ack: Ack = self.call("gsx_submitIntent", params).await?;
        let trimmed = ack
            .tx_hash
            .strip_prefix("0x")
            .or_else(|| ack.tx_hash.strip_prefix("0X"))
            .unwrap_or(&ack.tx_hash);
        let bytes =
            hex::decode(trimmed).map_err(|e| Error::Deserialize(format!("tx_hash hex: {}", e)))?;
        bytes.as_slice().try_into().map_err(|_| {
            Error::Deserialize(format!("tx_hash must be 32 bytes, got {}", bytes.len()))
        })
    }

    /// Sign and submit a typed intent in one call.
    ///
    /// Handles bincode serialization, digest computation, ML-DSA-65
    /// signing, and `signer_pubkey` derivation internally. Callers
    /// only need the typed `Intent`, their keypair, and the network id.
    ///
    /// Ring members (Authority/Validator) can pass `is_ring_member: true`
    /// to omit the public key from the submission (the server resolves
    /// it from the registry). Open signers must pass `false`.
    pub async fn submit_signed(
        &self,
        intent: &gsx_execution::Intent,
        sk: &gsx_crypto::mldsa::SecretKey,
        pk: &gsx_crypto::mldsa::PublicKey,
        network_id: &str,
        is_ring_member: bool,
    ) -> Result<[u8; 32], Error> {
        let intent_bytes = bincode::serde::encode_to_vec(intent, bincode::config::legacy())
            .map_err(|e| Error::Deserialize(format!("bincode encode: {e}")))?;
        let digest = gsx_execution::intent_signing_digest(network_id, &intent_bytes);
        let signature = gsx_crypto::mldsa::sign(&digest, sk)
            .map_err(|e| Error::Deserialize(format!("sign: {e:?}")))?;
        let pkh = *blake3::hash(pk.as_bytes()).as_bytes();
        let signer_pubkey = if is_ring_member {
            None
        } else {
            Some(pk.as_bytes())
        };
        self.submit_intent_raw(&intent_bytes, signature.as_bytes(), pkh, signer_pubkey)
            .await
    }

    /// Generic JSON-RPC call. Public so callers can drive any method
    /// that doesn't yet have a typed wrapper here.
    pub async fn call<T: DeserializeOwned>(&self, method: &str, params: Value) -> Result<T, Error> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        tracing::debug!(method, id, "gsx-client: sending request");

        let resp = self
            .http
            .post(&self.base_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Transport(e.to_string()))?
            .error_for_status()
            .map_err(|e| Error::Transport(e.to_string()))?;

        let envelope: JsonRpcResponse = resp
            .json()
            .await
            .map_err(|e| Error::Transport(format!("decode response: {}", e)))?;

        if let Some(err) = envelope.error {
            return Err(Error::Rpc {
                code: err.code,
                message: err.message,
            });
        }
        let result = envelope.result.ok_or(Error::MalformedResponse(
            "response carried neither result nor error",
        ))?;
        serde_json::from_value(result).map_err(|e| Error::Deserialize(e.to_string()))
    }
}

impl Clone for Client {
    fn clone(&self) -> Self {
        Self {
            http: self.http.clone(),
            base_url: self.base_url.clone(),
            next_id: self.next_id.clone(),
        }
    }
}

/// Return shape for [`Client::get_stake`].
#[derive(Debug, Clone, Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct StakeEntry {
    /// Authority id this entry describes.
    pub id: u32,
    /// Posted stake in GSX, encoded as a decimal string (u128 doesn't
    /// fit in a JSON number).
    pub stake_gsx: String,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    #[serde(rename = "jsonrpc")]
    _jsonrpc: String,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<JsonRpcErrorBody>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcErrorBody {
    code: i32,
    message: String,
}
