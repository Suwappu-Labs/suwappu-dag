//! suwappu-crypto — Suwappu DAG L1 cryptographic primitives.
//!
//! Implements the cryptographic-posture commitments of the *Suwappu DAG Layer 1*
//! paper:
//!
//! - §3.3 adversary model — post-quantum-conservative assumptions:
//!   - Existential unforgeability of ML-DSA-65 under chosen-message attack (FIPS 204)
//!   - IND-CCA2 security of ML-KEM-768 (FIPS 203)
//!   - Security of BLS12-381 aggregate signatures during the migration window
//!   - Collision resistance of SHA3-256 (FIPS 202) and Poseidon2
//! - §10.2 constant-size on-chain commitment composition:
//!   `ML-KEM-768 ciphertext (1,568 B) + BLS12-381 aggregate signature (96 B)
//!    + SHA3-256 payload root (32 B) ≈ 1,600 B`
//! - §12 classical-cryptography exception zones and migration targets:
//!   - EVM account TX signing: ECDSA secp256k1 hybrid-composed with ML-DSA-65,
//!     migrating to pure ML-DSA-65 by ~2030
//!   - LTP aggregate signatures: BLS12-381 → hash-based + SP1-STARK aggregation,
//!     2027–2029
//!   - Optional verification mode: Groth16 over BN254 → FRI default by ~2030
//!
//! All primitives expose **constant-time-safe** round-trip and verification
//! APIs over `&[u8]` payloads. Sprint exit gate is a 10,000-case property test
//! per primitive (see `tests/proptest_roundtrips.rs`).

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(rust_2018_idioms)]

pub mod bls;
pub mod error;
pub mod hash;
pub mod mldsa;
pub mod mlkem;

pub use error::CryptoError;

/// Cryptographic suite identifier embedded in domain separation tags.
pub const DOMAIN_TAG: &[u8] = b"Suwappu DAG-L1/v1";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_tag_is_versioned() {
        assert_eq!(DOMAIN_TAG, b"Suwappu DAG-L1/v1");
    }
}
