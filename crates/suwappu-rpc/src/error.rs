//! JSON-RPC error taxonomy. Codes follow the JSON-RPC 2.0 reserved range
//! (-32768..=-32000) plus application-level codes in -32099..=-32000.

use thiserror::Error;

/// C4 hardening: `#[non_exhaustive]` — external crates that match on
/// `RpcError` must include a wildcard arm. New error codes
/// (per-IP rate limit emission, future application-level rejection
/// reasons) become non-breaking additions.
#[derive(Debug, Error)]
#[non_exhaustive]
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

    /// `submit_intent`: signer_pubkey_hash didn't resolve to a seated
    /// Authority Ring member. Returned to the SDK so the wallet can
    /// surface "your key isn't authorized to write to this network" —
    /// distinct from MethodNotFound and NotFound so error UX can branch.
    #[error("unknown signer: {0}")]
    UnknownSigner(String),

    /// `submit_intent`: signature failed ML-DSA-65 verification against
    /// the resolved pubkey.
    #[error("bad signature: {0}")]
    BadSignature(String),

    /// `submit_intent`: daemon's intent channel is full or closed.
    /// Transient — caller should retry with backoff.
    #[error("enqueue full: {0}")]
    EnqueueFull(String),

    /// `submit_intent`: signature is valid but the signer's derived
    /// address does not match the intent's sender field.
    #[error("unauthorized: {0}")]
    Unauthorized(String),

    /// B2 hardening: the request was rejected by an ingress
    /// middleware (concurrency cap, body-size cap, or future
    /// per-IP rate limit). Transient — caller should retry with
    /// backoff. Mapped to code -32099 (Cloudflare-style 429
    /// analog within the JSON-RPC reserved-error range).
    #[error("rate limited: {0}")]
    RateLimited(String),
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
            RpcError::UnknownSigner(_) => -32001,
            RpcError::BadSignature(_) => -32002,
            RpcError::EnqueueFull(_) => -32003,
            RpcError::Unauthorized(_) => -32004,
            // -32099: explicitly chosen to occupy the OTHER end of
            // the application-level range so it can't collide with
            // a future NotFound-style addition.
            RpcError::RateLimited(_) => -32099,
        }
    }
}
