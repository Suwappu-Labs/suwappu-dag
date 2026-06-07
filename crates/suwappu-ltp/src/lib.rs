//! suwappu-ltp — Lattice Transfer Protocol integration.
//!
//! Implements §10 of the *Suwappu DAG Layer 1* paper. Companion paper:
//! *Lattice Transfer Protocol* (`suwappu-papers/papers/ltp`).
//!
//! - §10.1 bridgeless under a precise definition (no relayer custody, no
//!   third-party custodian, source-chain state verified)
//! - §10.2 constant-size on-chain commitment (≈1,600 B regardless of payload)
//!   - ML-KEM-768 sealed session key (≈1,568 B)
//!   - BLS12-381 aggregated signature (≈96 B)
//!   - SHA3-256 content-addressed payload root (32 B)
//! - §10.3 cross-chain DID synchronization (STARK pipeline SP1 on Plonky3,
//!   Goldilocks field, FRI-based commitment)
//!
//! Sprint scope:
//!
//! - DAG-S15: super-node attestation pipeline (7-of-9 quorum) ✅
//! - DAG-S16: Commitment Node DA SLA enforcement ✅
//! - DAG-S17: cross-chain DID STARK proof generation + verification ✅
//!
//! Super-node quorum tolerates a sub-quorum minority of compromised witnesses
//! without producing a forged attestation (paper §9, mitigation 1).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod attestation;
pub mod da;
pub mod did_stark;

pub use attestation::{
    attest, verify_attestation, AttestationPayload, AuthorityId, ChainId, Corridor,
    CorridorAttestation, CorridorId, LtpError, SuperNode, WitnessSignature,
};
pub use da::{verify_sla, Cid, Commitment, CommitmentNetwork, DaError, DaSla};
pub use did_stark::{
    prove_rotation, verify_rotation_proof, DidRotationStatement, DidStarkError, DidStarkProof,
};

/// LTP super-node attestation quorum (7-of-9).
pub const LTP_ATTESTATION_QUORUM_THRESHOLD: usize = 7;
/// LTP super-node attestation quorum size.
pub const LTP_ATTESTATION_QUORUM_SIZE: usize = 9;

/// Constant on-chain commitment size in bytes (paper §10.2).
pub const ON_CHAIN_COMMITMENT_BYTES: usize = 1_600;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quorum_matches_paper() {
        assert_eq!(LTP_ATTESTATION_QUORUM_THRESHOLD, 7);
        assert_eq!(LTP_ATTESTATION_QUORUM_SIZE, 9);
    }

    #[test]
    fn commitment_size_matches_paper() {
        assert_eq!(ON_CHAIN_COMMITMENT_BYTES, 1_600);
    }
}
