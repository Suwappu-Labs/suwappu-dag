//! axum router for JSON-RPC 2.0 over HTTP POST.
//!
//! Single endpoint `POST /` accepts a JSON-RPC request body and returns
//! the corresponding response. Non-POST methods → 405. Malformed JSON
//! → 200 with a JSON-RPC InvalidRequest error in the body (per spec —
//! HTTP status stays 200, transport errors are encoded in the response
//! envelope).

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::post, Json, Router};
use serde_json::Value;
use tracing::warn;

use crate::{
    context::{RpcContext, StateView},
    error::RpcError,
    methods,
    types::{JsonRpcRequest, JsonRpcResponse},
};

/// Build the axum router. Caller is responsible for `axum::serve`-ing it.
pub fn router<S: StateView>(ctx: Arc<RpcContext<S>>) -> Router {
    Router::new()
        .route("/", post(handle_rpc::<S>))
        .with_state(ctx)
}

async fn handle_rpc<S: StateView>(
    State(ctx): State<Arc<RpcContext<S>>>,
    body: Result<Json<JsonRpcRequest>, axum::extract::rejection::JsonRejection>,
) -> impl IntoResponse {
    let req = match body {
        Ok(Json(r)) => r,
        Err(err) => {
            warn!(error = %err, "rpc: malformed request body");
            // Per JSON-RPC 2.0, the id is "Null" when not parseable.
            let resp = JsonRpcResponse::err(Value::Null, RpcError::InvalidRequest(err.body_text()));
            return (StatusCode::OK, Json(resp));
        }
    };

    if req.jsonrpc != "2.0" {
        let resp = JsonRpcResponse::err(
            req.id.clone(),
            RpcError::InvalidRequest(format!(
                "jsonrpc field must be \"2.0\", got {:?}",
                req.jsonrpc
            )),
        );
        return (StatusCode::OK, Json(resp));
    }

    let result = dispatch(&*ctx.state, &req.method, &req.params).await;
    let resp = match result {
        Ok(value) => JsonRpcResponse::ok(req.id, value),
        Err(err) => JsonRpcResponse::err(req.id, err),
    };
    (StatusCode::OK, Json(resp))
}

/// Method-name → handler. New methods register here.
async fn dispatch<S: StateView>(
    state: &S,
    method: &str,
    params: &Value,
) -> Result<Value, RpcError> {
    match method {
        "gsx_getEpoch" => methods::get_epoch(state, params).await,
        "gsx_getAuthorityRegistry" => methods::get_authority_registry(state, params).await,
        "gsx_getValidatorRegistry" => methods::get_validator_registry(state, params).await,
        "gsx_getStake" => methods::get_stake(state, params).await,
        "gsx_getBalance" => methods::get_balance(state, params).await,
        "gsx_getBlock" => methods::get_block(state, params).await,
        "gsx_getTransaction" => methods::get_transaction(state, params).await,
        unknown => Err(RpcError::MethodNotFound(unknown.into())),
    }
}
