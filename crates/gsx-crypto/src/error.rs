//! Crypto error types.

use thiserror::Error;

/// Errors emitted by `gsx-crypto` primitives.
#[derive(Debug, Error)]
pub enum CryptoError {
    /// Signature verification failed.
    #[error("signature verification failed")]
    InvalidSignature,

    /// KEM decapsulation failed.
    #[error("kem decapsulation failed")]
    DecapsulationFailed,

    /// Public or secret key was malformed.
    #[error("malformed key: {0}")]
    MalformedKey(&'static str),

    /// Byte payload had unexpected size.
    #[error("byte payload size mismatch: expected {expected}, got {got}")]
    SizeMismatch {
        /// Expected size in bytes.
        expected: usize,
        /// Actual size in bytes.
        got: usize,
    },

    /// BLS aggregation produced an invalid result.
    #[error("bls aggregation invalid: {0}")]
    BlsAggregation(&'static str),
}
