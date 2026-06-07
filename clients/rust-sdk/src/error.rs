//! Error taxonomy for the `suwappu-client` SDK.

use thiserror::Error;

/// Errors surfaced by [`crate::Client`] method calls.
#[derive(Debug, Error)]
pub enum Error {
    /// TCP / TLS / HTTP-status-code failure. The string carries the
    /// underlying message; the SDK doesn't currently distinguish
    /// connection-refused from 5xx because callers usually want to
    /// retry both the same way.
    #[error("transport: {0}")]
    Transport(String),

    /// JSON-RPC application-level error response from the server. The
    /// `code` matches the JSON-RPC 2.0 reserved range plus suwappu-rpc's
    /// application codes (currently just -32000 NotFound).
    #[error("rpc error {code}: {message}")]
    Rpc {
        /// JSON-RPC error code.
        code: i32,
        /// Human-readable error message from the server.
        message: String,
    },

    /// The response envelope was syntactically valid JSON-RPC but
    /// carried neither a `result` nor an `error` field — protocol
    /// violation on the server side.
    #[error("malformed response: {0}")]
    MalformedResponse(&'static str),

    /// The `result` payload couldn't be deserialized into the typed
    /// return shape. Usually a version skew between client and server.
    #[error("deserialize: {0}")]
    Deserialize(String),
}
