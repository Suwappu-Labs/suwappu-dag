//! JSON-RPC method dispatch.
//!
//! Each method is an async free function over a generic `StateView`.
//! The router maps method-name strings to dispatch functions in
//! [`crate::router`]; method bodies stay focused on snapshot →
//! `serde_json::Value`.

use serde::Deserialize;
use serde_json::Value;

use crate::context::StateView;
use crate::error::RpcError;

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
