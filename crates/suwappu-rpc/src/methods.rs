//! JSON-RPC method dispatch.
//!
//! Each method is an async free function over a generic `StateView`.
//! The router maps method-name strings to dispatch functions in
//! [`crate::router()`]; method bodies stay focused on snapshot →
//! `serde_json::Value`.

use serde::Deserialize;
use serde_json::Value;

use crate::{context::StateView, error::RpcError};

/// `suwappu_getEpoch` — no params; returns `EpochView`.
pub async fn get_epoch<S: StateView>(state: &S, params: &Value) -> Result<Value, RpcError> {
    expect_no_params(params)?;
    let snap = state.epoch_snapshot().await;
    serde_json::to_value(snap).map_err(|e| RpcError::Internal(e.to_string()))
}

/// `suwappu_getAuthorityRegistry` — no params; returns ordered list of
/// `AuthorityMemberView`.
pub async fn get_authority_registry<S: StateView>(
    state: &S,
    params: &Value,
) -> Result<Value, RpcError> {
    expect_no_params(params)?;
    let snap = state.authority_snapshot().await;
    serde_json::to_value(snap).map_err(|e| RpcError::Internal(e.to_string()))
}

/// `suwappu_getValidatorRegistry` — no params; returns ordered list of
/// `ValidatorMemberView`.
pub async fn get_validator_registry<S: StateView>(
    state: &S,
    params: &Value,
) -> Result<Value, RpcError> {
    expect_no_params(params)?;
    let snap = state.validator_snapshot().await;
    serde_json::to_value(snap).map_err(|e| RpcError::Internal(e.to_string()))
}

#[derive(Deserialize)]
struct GetStakeParams {
    authority_id: u32,
}

/// `suwappu_getStake` — params `{ authority_id: u32 }`; returns
/// `{ id, stake_suwappu: String }` or NotFound.
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
            "stake_suwappu": stake.to_string(),
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

/// `suwappu_getBalance` — params `{ address: hex }` or positional `[hex]`.
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

/// `suwappu_getBlock` — params `{ round: u64 }` or positional `[u64]`;
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

/// `suwappu_getTransaction` — params `{ tx_hash: hex }` or positional;
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
    /// Bincode-serialized `suwappu_execution::Intent`, hex-encoded.
    /// Accepts with or without `0x` prefix.
    intent: String,
    /// ML-DSA-65 signature over `intent_signing_digest(network_id, intent)`,
    /// hex-encoded.
    signature: String,
    /// `blake3(public_key_bytes)`, 32 bytes hex.
    signer_pubkey_hash: String,
    /// ML-DSA-65 public key bytes, hex-encoded. Required for open
    /// signers (callers not in the Authority or Validator Ring).
    /// Ring members may omit — their pubkey is resolved from the
    /// registry via `signer_pubkey_hash`.
    #[serde(default)]
    signer_pubkey: Option<String>,
}

/// `suwappu_submitIntent` — params `{intent: hex, signature: hex,
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
            Value::Array(arr) if arr.len() == 3 || arr.len() == 4 => SubmitIntentParams {
                intent: serde_json::from_value(arr[0].clone())
                    .map_err(|e| RpcError::InvalidParams(e.to_string()))?,
                signature: serde_json::from_value(arr[1].clone())
                    .map_err(|e| RpcError::InvalidParams(e.to_string()))?,
                signer_pubkey_hash: serde_json::from_value(arr[2].clone())
                    .map_err(|e| RpcError::InvalidParams(e.to_string()))?,
                signer_pubkey: arr.get(3).map(|v| {
                    serde_json::from_value(v.clone())
                }).transpose()
                    .map_err(|e| RpcError::InvalidParams(e.to_string()))?,
            },
            _ => return Err(RpcError::InvalidParams(
                "expected `{intent, signature, signer_pubkey_hash, [signer_pubkey]}` (all hex) or 3-4 element array"
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
    let signer_pubkey = p
        .signer_pubkey
        .as_deref()
        .map(|s| decode_hex_field("signer_pubkey", s))
        .transpose()?;

    use crate::context::SubmitIntentError;
    match state
        .submit_intent(intent_bytes, signature, pkh, signer_pubkey)
        .await
    {
        Ok(hash) => Ok(serde_json::json!({
            "tx_hash": format!("0x{}", hex::encode(hash)),
        })),
        Err(SubmitIntentError::BadIntentEncoding(msg)) => {
            Err(RpcError::InvalidParams(format!("intent decode: {}", msg)))
        }
        Err(SubmitIntentError::UnknownSigner) => Err(RpcError::UnknownSigner(
            "signer not in Authority/Validator Ring and no valid signer_pubkey provided".into(),
        )),
        Err(SubmitIntentError::BadSignature) => {
            Err(RpcError::BadSignature("ML-DSA-65 verify failed".into()))
        }
        Err(SubmitIntentError::EnqueueFull) => Err(RpcError::EnqueueFull(
            "intent channel full or closed; retry".into(),
        )),
        Err(SubmitIntentError::Unauthorized) => Err(RpcError::Unauthorized(
            "signer address does not match intent sender".into(),
        )),
    }
}

/// `suwappu_getL1StateRoot` — no params; returns `{ state_root: "0x..." }`.
/// The L2 sequencer daemon reads this as `prev_l1_state_root` for
/// each batch header, binding the L2 proof to a specific L1 height.
pub async fn get_l1_state_root<S: StateView>(state: &S, params: &Value) -> Result<Value, RpcError> {
    expect_no_params(params)?;
    let root = state.l1_state_root().await;
    Ok(serde_json::json!({
        "state_root": format!("0x{}", hex::encode(root)),
    }))
}

#[derive(Deserialize)]
struct GetL2StateRootParams {
    /// BLAKE3("suwappu-l2-chain-" || chain_id), hex-encoded. Identifies
    /// which L2 chain's state root to return.
    l2_chain_id_hash: String,
}

/// `suwappu_getL2StateRoot` — params `{ l2_chain_id_hash: hex }` or
/// positional `[hex]`; returns `{ state_root: "0x..." }`. Returns
/// all-zeros if no L2 state-root commit has landed for this chain.
pub async fn get_l2_state_root<S: StateView>(state: &S, params: &Value) -> Result<Value, RpcError> {
    let p: GetL2StateRootParams = match params {
        Value::Object(_) => serde_json::from_value(params.clone())
            .map_err(|e| RpcError::InvalidParams(e.to_string()))?,
        Value::Array(arr) if arr.len() == 1 => {
            let h: String = serde_json::from_value(arr[0].clone())
                .map_err(|e| RpcError::InvalidParams(e.to_string()))?;
            GetL2StateRootParams {
                l2_chain_id_hash: h,
            }
        }
        _ => {
            return Err(RpcError::InvalidParams(
                "expected `{l2_chain_id_hash: hex}` or `[hex]`".into(),
            ))
        }
    };

    let trimmed = p
        .l2_chain_id_hash
        .strip_prefix("0x")
        .or_else(|| p.l2_chain_id_hash.strip_prefix("0X"))
        .unwrap_or(&p.l2_chain_id_hash);
    let bytes = hex::decode(trimmed)
        .map_err(|e| RpcError::InvalidParams(format!("l2_chain_id_hash hex: {}", e)))?;
    let hash: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
        RpcError::InvalidParams(format!(
            "l2_chain_id_hash must be 32 bytes, got {}",
            bytes.len()
        ))
    })?;

    let root = state.l2_state_root(hash).await;
    Ok(serde_json::json!({
        "state_root": format!("0x{}", hex::encode(root)),
    }))
}

/// `suwappu_getForceIncludeRegistry` — no params; returns `{ data: "0x..." }`.
/// The L2 sequencer daemon decodes the raw bytes via
/// `suwappu_execution::force_include::decode_map` to discover pending
/// force-include obligations.
pub async fn get_force_include_registry<S: StateView>(
    state: &S,
    params: &Value,
) -> Result<Value, RpcError> {
    expect_no_params(params)?;
    let bytes = state.force_include_registry_bytes().await;
    Ok(serde_json::json!({
        "data": format!("0x{}", hex::encode(bytes)),
    }))
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
