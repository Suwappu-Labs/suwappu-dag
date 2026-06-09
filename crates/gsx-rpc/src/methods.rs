//! JSON-RPC method dispatch.
//!
//! Each method is an async free function over a generic `StateView`.
//! The router maps method-name strings to dispatch functions in
//! [`crate::router()`]; method bodies stay focused on snapshot →
//! `serde_json::Value`.

use serde::Deserialize;
use serde_json::Value;

use crate::{context::StateView, error::RpcError};

/// `gsx_getEpoch` — no params; returns `EpochView`.
pub async fn get_epoch<S: StateView>(state: &S, params: &Value) -> Result<Value, RpcError> {
    expect_no_params(params)?;
    let snap = state.epoch_snapshot().await;
    serde_json::to_value(snap).map_err(|e| RpcError::Internal(e.to_string()))
}

/// `gsx_getAuthorityRegistry` — no params; returns ordered list of
/// `AuthorityMemberView`.
pub async fn get_authority_registry<S: StateView>(
    state: &S,
    params: &Value,
) -> Result<Value, RpcError> {
    expect_no_params(params)?;
    let snap = state.authority_snapshot().await;
    serde_json::to_value(snap).map_err(|e| RpcError::Internal(e.to_string()))
}

/// `gsx_getValidatorRegistry` — no params; returns ordered list of
/// `ValidatorMemberView`.
pub async fn get_validator_registry<S: StateView>(
    state: &S,
    params: &Value,
) -> Result<Value, RpcError> {
    expect_no_params(params)?;
    let snap = state.validator_snapshot().await;
    serde_json::to_value(snap).map_err(|e| RpcError::Internal(e.to_string()))
}

/// `gsx_getHeaderAttestation` — no params; returns this node's signed
/// bridge-header side-attestation over its latest finalized block as a
/// `HeaderAttestationView`, or JSON `null` if no block has finalized yet or the
/// node has no bridge signer configured. A relayer polls this on every
/// validator and aggregates a >2/3-stake set for
/// `GsxDagQuorumHeaderOracle.submitHeader`.
pub async fn get_header_attestation<S: StateView>(
    state: &S,
    params: &Value,
) -> Result<Value, RpcError> {
    expect_no_params(params)?;
    match state.header_attestation().await {
        Some(att) => serde_json::to_value(att).map_err(|e| RpcError::Internal(e.to_string())),
        None => Ok(Value::Null),
    }
}

#[derive(Deserialize)]
struct GetStakeParams {
    authority_id: u32,
}

/// `gsx_getStake` — params `{ authority_id: u32 }`; returns
/// `{ id, stake_gsx: String }` or NotFound.
pub async fn get_stake<S: StateView>(state: &S, params: &Value) -> Result<Value, RpcError> {
    let p: GetStakeParams = match params {
        Value::Object(_) => serde_json::from_value(params.clone())
            .map_err(|e| RpcError::InvalidParams(e.to_string()))?,
        Value::Array(arr) if arr.len() == 1 => {
            let id: u32 = serde_json::from_value(arr[0].clone())
                .map_err(|e| RpcError::InvalidParams(e.to_string()))?;
            GetStakeParams { authority_id: id }
        }
        _ => {
            return Err(RpcError::InvalidParams(
                "expected `{authority_id: u32}` or `[u32]`".into(),
            ))
        }
    };

    match state.stake_for(p.authority_id).await {
        Some(stake) => Ok(serde_json::json!({
            "id": p.authority_id,
            "stake_gsx": stake.to_string(),
        })),
        None => Err(RpcError::NotFound(format!(
            "no stake recorded for authority_id {}",
            p.authority_id
        ))),
    }
}

#[derive(Deserialize)]
struct GetBalanceParams {
    /// Hex-encoded 20-byte address. Accepts with or without `0x` prefix
    /// (case-insensitive). Anything that doesn't decode to exactly
    /// 20 bytes returns InvalidParams.
    address: String,
}

/// `gsx_getBalance` — params `{ address: hex }` or positional `[hex]`.
/// Returns `{ address: "0x..", balance: "<decimal>" }`. A zero balance
/// is a valid response (the substrate doesn't distinguish "absent"
/// from "explicit zero") — clients should not interpret it as NotFound.
pub async fn get_balance<S: StateView>(state: &S, params: &Value) -> Result<Value, RpcError> {
    let p: GetBalanceParams = match params {
        Value::Object(_) => serde_json::from_value(params.clone())
            .map_err(|e| RpcError::InvalidParams(e.to_string()))?,
        Value::Array(arr) if arr.len() == 1 => {
            let hex_addr: String = serde_json::from_value(arr[0].clone())
                .map_err(|e| RpcError::InvalidParams(e.to_string()))?;
            GetBalanceParams { address: hex_addr }
        }
        _ => {
            return Err(RpcError::InvalidParams(
                "expected `{address: hex}` or `[hex]`".into(),
            ))
        }
    };

    let trimmed = p
        .address
        .strip_prefix("0x")
        .or_else(|| p.address.strip_prefix("0X"))
        .unwrap_or(&p.address);
    let bytes =
        hex::decode(trimmed).map_err(|e| RpcError::InvalidParams(format!("address hex: {}", e)))?;
    let addr: [u8; 20] = bytes.as_slice().try_into().map_err(|_| {
        RpcError::InvalidParams(format!("address must be 20 bytes, got {}", bytes.len()))
    })?;

    let balance = state.balance_for(addr).await;
    Ok(serde_json::json!({
        "address": format!("0x{}", hex::encode(addr)),
        "balance": balance.to_string(),
    }))
}

#[derive(Deserialize)]
struct GetBlockParams {
    round: u64,
}

/// `gsx_getBlock` — params `{ round: u64 }` or positional `[u64]`;
/// returns `BlockView` or NotFound if no block has been committed at
/// that round.
pub async fn get_block<S: StateView>(state: &S, params: &Value) -> Result<Value, RpcError> {
    let p: GetBlockParams = match params {
        Value::Object(_) => serde_json::from_value(params.clone())
            .map_err(|e| RpcError::InvalidParams(e.to_string()))?,
        Value::Array(arr) if arr.len() == 1 => {
            let r: u64 = serde_json::from_value(arr[0].clone())
                .map_err(|e| RpcError::InvalidParams(e.to_string()))?;
            GetBlockParams { round: r }
        }
        _ => {
            return Err(RpcError::InvalidParams(
                "expected `{round: u64}` or `[u64]`".into(),
            ))
        }
    };

    match state.block_at_round(p.round).await {
        Some(view) => serde_json::to_value(view).map_err(|e| RpcError::Internal(e.to_string())),
        None => Err(RpcError::NotFound(format!(
            "no committed block at round {}",
            p.round
        ))),
    }
}

#[derive(Deserialize)]
struct GetTransactionParams {
    /// 32-byte intent hash, hex-encoded. Accepts with or without `0x`
    /// prefix (case-insensitive).
    tx_hash: String,
}

/// `gsx_getTransaction` — params `{ tx_hash: hex }` or positional;
/// returns `TransactionView` or NotFound if the hash has never been
/// observed in a committed block.
pub async fn get_transaction<S: StateView>(state: &S, params: &Value) -> Result<Value, RpcError> {
    let p: GetTransactionParams = match params {
        Value::Object(_) => serde_json::from_value(params.clone())
            .map_err(|e| RpcError::InvalidParams(e.to_string()))?,
        Value::Array(arr) if arr.len() == 1 => {
            let h: String = serde_json::from_value(arr[0].clone())
                .map_err(|e| RpcError::InvalidParams(e.to_string()))?;
            GetTransactionParams { tx_hash: h }
        }
        _ => {
            return Err(RpcError::InvalidParams(
                "expected `{tx_hash: hex}` or `[hex]`".into(),
            ))
        }
    };

    let trimmed = p
        .tx_hash
        .strip_prefix("0x")
        .or_else(|| p.tx_hash.strip_prefix("0X"))
        .unwrap_or(&p.tx_hash);
    let bytes =
        hex::decode(trimmed).map_err(|e| RpcError::InvalidParams(format!("tx_hash hex: {}", e)))?;
    let hash: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
        RpcError::InvalidParams(format!("tx_hash must be 32 bytes, got {}", bytes.len()))
    })?;

    match state.transaction_by_hash(hash).await {
        Some(view) => serde_json::to_value(view).map_err(|e| RpcError::Internal(e.to_string())),
        None => Err(RpcError::NotFound(format!(
            "no committed transaction with hash 0x{}",
            hex::encode(hash)
        ))),
    }
}

#[derive(Deserialize)]
struct SubmitIntentParams {
    /// Bincode-serialized `gsx_execution::Intent`, hex-encoded.
    /// Accepts with or without `0x` prefix.
    intent: String,
    /// ML-DSA-65 signature over `intent_signing_digest(network_id, intent)`,
    /// hex-encoded.
    signature: String,
    /// `blake3(public_key_bytes)`, 32 bytes hex.
    signer_pubkey_hash: String,
}

/// `gsx_submitIntent` — params `{intent: hex, signature: hex,
/// signer_pubkey_hash: hex}` or positional `[intent, signature, hash]`.
/// Returns `{tx_hash: 0x..}` on accept.
///
/// The SDK is expected to bincode-serialize the typed Intent locally
/// (same wire form the TCP/bincode path uses) so the same signed
/// payload works through either ingress wire.
pub async fn submit_intent<S: StateView>(state: &S, params: &Value) -> Result<Value, RpcError> {
    let p: SubmitIntentParams =
        match params {
            Value::Object(_) => serde_json::from_value(params.clone())
                .map_err(|e| RpcError::InvalidParams(e.to_string()))?,
            Value::Array(arr) if arr.len() == 3 => SubmitIntentParams {
                intent: serde_json::from_value(arr[0].clone())
                    .map_err(|e| RpcError::InvalidParams(e.to_string()))?,
                signature: serde_json::from_value(arr[1].clone())
                    .map_err(|e| RpcError::InvalidParams(e.to_string()))?,
                signer_pubkey_hash: serde_json::from_value(arr[2].clone())
                    .map_err(|e| RpcError::InvalidParams(e.to_string()))?,
            },
            _ => return Err(RpcError::InvalidParams(
                "expected `{intent, signature, signer_pubkey_hash}` (all hex) or 3-element array"
                    .into(),
            )),
        };

    let intent_bytes = decode_hex_field("intent", &p.intent)?;
    let signature = decode_hex_field("signature", &p.signature)?;
    let pkh_bytes = decode_hex_field("signer_pubkey_hash", &p.signer_pubkey_hash)?;
    let pkh: [u8; 32] = pkh_bytes.as_slice().try_into().map_err(|_| {
        RpcError::InvalidParams(format!(
            "signer_pubkey_hash must be 32 bytes, got {}",
            pkh_bytes.len()
        ))
    })?;

    use crate::context::SubmitIntentError;
    match state.submit_intent(intent_bytes, signature, pkh).await {
        Ok(hash) => Ok(serde_json::json!({
            "tx_hash": format!("0x{}", hex::encode(hash)),
        })),
        Err(SubmitIntentError::BadIntentEncoding(msg)) => {
            Err(RpcError::InvalidParams(format!("intent decode: {}", msg)))
        }
        Err(SubmitIntentError::UnknownSigner) => Err(RpcError::UnknownSigner(
            "signer_pubkey_hash not in Authority Ring".into(),
        )),
        Err(SubmitIntentError::BadSignature) => {
            Err(RpcError::BadSignature("ML-DSA-65 verify failed".into()))
        }
        Err(SubmitIntentError::EnqueueFull) => Err(RpcError::EnqueueFull(
            "intent channel full or closed; retry".into(),
        )),
    }
}

/// Strip optional `0x` / `0X` prefix and hex-decode. Used by every
/// hex-bearing param path in this module.
fn decode_hex_field(field: &str, value: &str) -> Result<Vec<u8>, RpcError> {
    let trimmed = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    hex::decode(trimmed).map_err(|e| RpcError::InvalidParams(format!("{} hex: {}", field, e)))
}

fn expect_no_params(params: &Value) -> Result<(), RpcError> {
    match params {
        Value::Null => Ok(()),
        Value::Array(arr) if arr.is_empty() => Ok(()),
        Value::Object(o) if o.is_empty() => Ok(()),
        _ => Err(RpcError::InvalidParams(
            "this method takes no parameters".into(),
        )),
    }
}
