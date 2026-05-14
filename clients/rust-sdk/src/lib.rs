//! `gsx-client` — Rust client SDK for the gsx-dag JSON-RPC query API.
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
//! ```no_run
//! # async fn doc() -> Result<(), gsx_client::Error> {
//! let client = gsx_client::Client::new("http://127.0.0.1:9092");
//! let epoch = client.get_epoch().await?;
//! println!(
//!     "epoch={} rounds_per_epoch={} last_boundary_round={}",
//!     epoch.current, epoch.rounds_per_epoch, epoch.last_boundary_round
//! );
//! # Ok(())
//! # }
//! ```
//!
//! Construction returns `Client` directly (no `Result`) — TCP reachability
//! is deferred to the first method call, which surfaces transport errors
//! as [`Error::Transport`].

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod error;

use std::sync::atomic::{AtomicU64, Ordering};

pub use error::Error;
pub use gsx_rpc::context::{AuthorityMemberView, BalanceView, EpochView, ValidatorMemberView};
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
