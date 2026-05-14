//! JSON-RPC error taxonomy. Codes follow the JSON-RPC 2.0 reserved range
//! (-32768..=-32000) plus application-level codes in -32099..=-32000.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum RpcError {
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("method not found: {0}")]
    MethodNotFound(String),

    #[error("invalid params: {0}")]
    InvalidParams(String),

    #[error("internal error: {0}")]
    Internal(String),

    #[error("not found: {0}")]
    NotFound(String),
}

impl RpcError {
    /// JSON-RPC error code.
    pub fn code(&self) -> i32 {
        match self {
            RpcError::InvalidRequest(_) => -32600,
            RpcError::MethodNotFound(_) => -32601,
            RpcError::InvalidParams(_) => -32602,
            RpcError::Internal(_) => -32603,
            // Application-level codes start at -32000.
            RpcError::NotFound(_) => -32000,
        }
    }
}
