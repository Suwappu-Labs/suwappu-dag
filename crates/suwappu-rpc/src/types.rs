//! JSON-RPC 2.0 wire types.
//!
//! Minimal subset of the spec (<https://www.jsonrpc.org/specification>):
//! single requests/responses only, no batch, no notifications.

use serde::{Deserialize, Serialize};

use crate::error::RpcError;

/// JSON-RPC 2.0 request envelope.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JsonRpcRequest {
    /// MUST be `"2.0"`.
    pub jsonrpc: String,
    /// Identifier echoed in the response. May be a number, string, or null.
    pub id: serde_json::Value,
    /// Method name (e.g. `"suwappu_getEpoch"`).
    pub method: String,
    /// Method parameters. May be omitted for parameterless methods.
    #[serde(default)]
    pub params: serde_json::Value,
}

/// JSON-RPC 2.0 response envelope.
///
/// Per spec, exactly one of `result` or `error` is present.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcErrorBody>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcErrorBody {
    pub code: i32,
    pub message: String,
}

impl JsonRpcResponse {
    pub fn ok(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: serde_json::Value, err: RpcError) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcErrorBody {
                code: err.code(),
                message: err.to_string(),
            }),
        }
    }
}
