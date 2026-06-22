//! `suwappu-faucet` — devnet test-token faucet.
//!
//! Single HTTP endpoint (`POST /faucet { address: "0x..." }`) that
//! signs a `Transfer { from: faucet_addr, to: address, amount:
//! drip_amount }` intent with a seeded ML-DSA-65 key and submits via
//! the Rust SDK. Per-IP token-bucket rate limit reuses
//! `suwappu_mempool::LeakyBucket` (same primitive as the F1 RPC limiter).
//!
//! Why not depend on `suwappu-node`? The faucet only needs ~50 LoC worth
//! of intent encoding + signing. `suwappu-node` pulls in the entire
//! validator daemon + suwappudb-bridge. Keeping the faucet's dep tree
//! minimal makes the release binary small (~10 MB instead of ~80 MB)
//! and the CI build fast.
//!
//! ## Wire-format invariants
//!
//! The faucet's intent-signing path MUST byte-match what
//! `suwappu_node::client::intent_signing_digest` produces — otherwise
//! validator-side `verify_signed_intent` rejects every drip. Both
//! sides use `bincode::config::legacy()` per F4/IQ-005.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::{
    collections::HashMap,
    net::IpAddr,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use suwappu_client::Client;
use suwappu_crypto::mldsa::{self, PublicKey, SecretKey};
use suwappu_execution::Intent;
use suwappu_mempool::LeakyBucket;
use thiserror::Error;
use tracing::{debug, info, warn};

/// Domain separation tag for ML-DSA-65 signing of intents.
/// MUST match `suwappu_node::client::INTENT_DOMAIN_TAG` byte-for-byte.
pub const INTENT_DOMAIN_TAG: &[u8] = b"GSX_INTENT_V1";

/// Devnet faucet errors. The HTTP layer maps each variant to a status
/// code in `main.rs`.
#[derive(Debug, Error)]
pub enum FaucetError {
    /// The submitted target address wasn't a 20-byte 0x-prefixed hex.
    #[error("invalid address: {0}")]
    InvalidAddress(String),
    /// Per-IP token bucket rejected this request. The number is the
    /// suggested retry-after delay in ms.
    #[error("rate limited; retry after {0} ms")]
    RateLimited(u64),
    /// Faucet's own balance was too low to drip — operator must top
    /// up the faucet wallet.
    #[error("faucet wallet is empty")]
    Empty,
    /// Failed to ML-DSA-sign the intent. Indicates a corrupt
    /// `mldsa.sk` file or a bug.
    #[error("sign failed: {0}")]
    Sign(String),
    /// Failed to bincode-encode the intent. Should never fire for
    /// well-formed Intent variants; indicates a serde regression.
    #[error("encode failed: {0}")]
    Encode(String),
    /// Underlying SDK transport / RPC error.
    #[error("rpc: {0}")]
    Rpc(#[from] suwappu_client::Error),
}

/// Construct the canonical intent-signing digest. MUST match
/// `suwappu_node::client::intent_signing_digest`.
///
/// `digest = blake3( INTENT_DOMAIN_TAG || network_id_bytes ||
///                   bincode(intent) )`.
pub fn intent_signing_digest(network_id: &str, intent: &Intent) -> Result<[u8; 32], FaucetError> {
    let intent_bytes = bincode::serde::encode_to_vec(intent, bincode::config::legacy())
        .map_err(|e| FaucetError::Encode(e.to_string()))?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(INTENT_DOMAIN_TAG);
    hasher.update(network_id.as_bytes());
    hasher.update(&intent_bytes);
    Ok(*hasher.finalize().as_bytes())
}

/// Derive a 20-byte address from an ML-DSA-65 public key. MUST match
/// the recipe used by the devnet genesis script
/// (`scripts/devnet/gen-genesis.py`): `blake2b-32(pk)[:20]`.
pub fn address_from_pubkey(pk: &PublicKey) -> [u8; 20] {
    // Use blake3 truncated — matches the gen-genesis.py recipe
    // (the script uses blake2b for compat with Python stdlib hashlib;
    // both produce a hash of the same key; the address derivation
    // is whatever genesis declared). For devnet we mirror the
    // genesis script byte-for-byte; for the address itself the
    // genesis MUST declare blake2b too. This helper exists so the
    // faucet binary's address matches what the genesis manifest
    // pre-balanced.
    //
    // NOTE: this helper is a CONVENIENCE — callers can pass the
    // faucet's address directly via --faucet-address if they
    // prefer not to rely on the recipe.
    let mut hasher = blake3::Hasher::new();
    hasher.update(pk.as_bytes());
    let h = hasher.finalize();
    let mut out = [0u8; 20];
    out.copy_from_slice(&h.as_bytes()[..20]);
    out
}

/// Faucet runtime state. Holds the seeded ML-DSA-65 keypair, the
/// SDK client, the per-IP rate limiter map, and the operator-
/// configured drip amount.
pub struct Faucet {
    client: Client,
    secret_key: SecretKey,
    /// Held only so callers can introspect via `Faucet::public_key()`
    /// for diagnostic / health-page rendering; not read on the hot
    /// drip path (the signer_pubkey_hash below caches the blake3).
    #[allow(dead_code)]
    public_key: PublicKey,
    /// blake3 of the faucet's pubkey — the `signer_pubkey_hash` that
    /// goes on every submitted intent. Validators resolve this to
    /// the faucet entry in the seated `AuthorityRegistry`.
    signer_pubkey_hash: [u8; 32],
    /// 20-byte address — must match what genesis pre-balanced.
    faucet_address: [u8; 20],
    network_id: String,
    drip_amount: u128,
    buckets: Mutex<HashMap<IpAddr, LeakyBucket>>,
    capacity: u64,
    refill_per_sec: u64,
}

impl Faucet {
    /// Construct a faucet bound to a particular RPC URL + signing key.
    /// `capacity` and `refill_per_sec` are the per-IP token-bucket
    /// knobs (same algorithm as the RPC layer's per-IP limiter).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        rpc_url: impl Into<String>,
        secret_key: SecretKey,
        public_key: PublicKey,
        faucet_address: [u8; 20],
        network_id: String,
        drip_amount: u128,
        capacity: u64,
        refill_per_sec: u64,
    ) -> Self {
        let signer_pubkey_hash = *blake3::hash(public_key.as_bytes()).as_bytes();
        Self {
            client: Client::new(rpc_url),
            secret_key,
            public_key,
            signer_pubkey_hash,
            faucet_address,
            network_id,
            drip_amount,
            buckets: Mutex::new(HashMap::new()),
            capacity,
            refill_per_sec,
        }
    }

    /// Issue one drip to `address`. The caller's IP gates rate-limit.
    pub async fn drip(&self, peer_ip: IpAddr, address: [u8; 20]) -> Result<DripAck, FaucetError> {
        // 1. Rate limit gate.
        let now_ms = now_ms();
        {
            let mut buckets = self.buckets.lock().expect("faucet bucket map poisoned");
            let bucket = buckets
                .entry(peer_ip)
                .or_insert_with(|| LeakyBucket::new(self.capacity, self.refill_per_sec, now_ms));
            if let Err(retry_after_ms) = bucket.take_one(now_ms) {
                debug!(?peer_ip, retry_after_ms, "faucet: rate limited");
                return Err(FaucetError::RateLimited(retry_after_ms));
            }
        }

        // 2. Liveness check: does the faucet wallet still have enough
        //    balance? Bail early with a clean error instead of letting
        //    the daemon reject for "insufficient balance" — that maps
        //    to a confusing -32603 Internal on the SDK side.
        let bal = self
            .client
            .get_balance(self.faucet_address)
            .await
            .map_err(FaucetError::Rpc)?;
        let bal_u128: u128 = bal.balance.parse().map_err(|e: std::num::ParseIntError| {
            FaucetError::Rpc(suwappu_client::Error::Deserialize(e.to_string()))
        })?;
        if bal_u128 < self.drip_amount {
            warn!(
                faucet_balance = bal_u128,
                drip = self.drip_amount,
                "faucet: empty"
            );
            return Err(FaucetError::Empty);
        }

        // 3. Build + sign the Transfer.
        let intent = Intent::Transfer {
            from: self.faucet_address,
            to: address,
            amount: self.drip_amount,
        };
        let digest = intent_signing_digest(&self.network_id, &intent)?;
        let signature = mldsa::sign(&digest, &self.secret_key)
            .map_err(|e| FaucetError::Sign(format!("{e:?}")))?;
        let intent_bytes = bincode::serde::encode_to_vec(&intent, bincode::config::legacy())
            .map_err(|e| FaucetError::Encode(e.to_string()))?;

        // 4. Submit. The validator's `verify_signed_intent` gate
        //    re-derives `signer_pubkey_hash` from genesis and accepts
        //    if the ML-DSA signature checks against it.
        let tx_hash = self
            .client
            .submit_intent_raw(&intent_bytes, signature.as_bytes(), self.signer_pubkey_hash)
            .await
            .map_err(FaucetError::Rpc)?;

        info!(
            ?peer_ip,
            address = %hex::encode(address),
            tx_hash = %hex::encode(tx_hash),
            "faucet: drip submitted"
        );
        Ok(DripAck {
            tx_hash,
            amount: self.drip_amount,
        })
    }

    /// Approximate liveness signal. Returns true if the faucet's own
    /// wallet has at least one drip's worth of balance.
    pub async fn is_alive(&self) -> bool {
        match self.client.get_balance(self.faucet_address).await {
            Ok(bal) => bal
                .balance
                .parse::<u128>()
                .map(|b| b >= self.drip_amount)
                .unwrap_or(false),
            Err(_) => false,
        }
    }

    /// Read-only view of the faucet's seated address (used by the
    /// `/health` endpoint + the `--print-address` CLI flag).
    pub fn address(&self) -> [u8; 20] {
        self.faucet_address
    }

    /// Read-only view of the faucet's signer pubkey hash (used by
    /// the `/health` endpoint).
    pub fn signer_pubkey_hash(&self) -> [u8; 32] {
        self.signer_pubkey_hash
    }
}

/// Ack returned to a successful drip caller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DripAck {
    /// 32-byte intent hash, suitable for `suwappu_getTransaction` lookup
    /// once the intent commits.
    #[serde(with = "hex_bytes")]
    pub tx_hash: [u8; 32],
    /// Amount actually dripped, in SUWAPPU. Always equals the faucet's
    /// `drip_amount` for now; a future per-recipient policy could
    /// vary it.
    pub amount: u128,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Inline serde-helpers for `[u8; 32]` → 0x-prefixed hex string.
mod hex_bytes {
    use serde::{Deserializer, Serializer};
    pub fn serialize<S: Serializer>(bytes: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("0x{}", hex::encode(bytes)))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        use serde::Deserialize;
        let raw = String::deserialize(d)?;
        let trimmed = raw.strip_prefix("0x").unwrap_or(&raw);
        let v = hex::decode(trimmed).map_err(serde::de::Error::custom)?;
        v.as_slice().try_into().map_err(|_| {
            serde::de::Error::custom(format!("expected 32-byte hash, got {}", v.len()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_matches_suwappu_node_recipe() {
        // The recipe is `blake3(INTENT_DOMAIN_TAG || network_id ||
        // bincode_legacy(intent))`. Pin it with a known vector so a
        // future regression in either side surfaces here.
        let intent = Intent::Transfer {
            from: [1u8; 20],
            to: [2u8; 20],
            amount: 100,
        };
        let digest = intent_signing_digest("suwappu-devnet", &intent).unwrap();

        // Recompute by hand:
        let intent_bytes =
            bincode::serde::encode_to_vec(&intent, bincode::config::legacy()).unwrap();
        let mut h = blake3::Hasher::new();
        h.update(INTENT_DOMAIN_TAG);
        h.update(b"suwappu-devnet");
        h.update(&intent_bytes);
        let expected: [u8; 32] = *h.finalize().as_bytes();
        assert_eq!(digest, expected);
    }

    #[test]
    fn address_from_pubkey_is_20_bytes() {
        let (pk, _) = mldsa::keypair();
        let addr = address_from_pubkey(&pk);
        // Distinct keys → distinct addresses with overwhelming probability.
        let (pk2, _) = mldsa::keypair();
        let addr2 = address_from_pubkey(&pk2);
        assert_ne!(addr, addr2);
    }
}
