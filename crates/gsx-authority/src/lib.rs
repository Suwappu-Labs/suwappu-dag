//! gsx-authority — Authority Ring (Proof-of-Authority).
//!
//! Implements §5.1 of the *GSX DAG Layer 1* paper:
//!
//! - 30–50 licensed institutional entities
//! - Authority Nodes produce certificates into the DAG
//! - Sign compliance attestations
//! - Participate in the fast-path quorum
//! - 100,000 GSX stake threshold per member
//! - Admission via Authority-Phase Matrix (paper §14)
//!
//! Sprint scope:
//!
//! - DAG-S6: admission state machine, registry, quorum threshold math ✅
//! - DAG-S7: equivocation detection, slashing (100% bonded stake + expulsion)
//!
//! Quorum: |Q_A| > (2/3)|A| (Definition 2).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod registry;

pub use registry::{AdmissionError, AuthorityMember, AuthorityRegistry};

/// Authority Ring participant identifier — index into the published set.
pub type AuthorityId = u32;

/// Stake threshold per Authority Node, in GSX.
pub const AUTHORITY_STAKE_THRESHOLD_GSX: u64 = 100_000;

/// Minimum Authority Ring size.
pub const AUTHORITY_RING_MIN: usize = 30;

/// Maximum Authority Ring size.
pub const AUTHORITY_RING_MAX: usize = 50;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_size_bounds_match_paper() {
        assert_eq!(AUTHORITY_RING_MIN, 30);
        assert_eq!(AUTHORITY_RING_MAX, 50);
    }

    #[test]
    fn stake_threshold_matches_paper() {
        assert_eq!(AUTHORITY_STAKE_THRESHOLD_GSX, 100_000);
    }
}
