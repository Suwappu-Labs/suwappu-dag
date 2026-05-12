//! gsx-validator — Validator Ring (Proof-of-Stake).
//!
//! Implements §5.2 of the *GSX DAG Layer 1* paper:
//!
//! - 100–500 stake-weighted open participants
//! - Validators ratify ordering, vote on Mysticeti commit rounds, enforce slashing
//! - 25,000 GSX genesis stake threshold per member
//! - Open admission subject to stake threshold, uptime, and key-management standard
//!
//! Sprint scope:
//!
//! - DAG-S6: stake registry, stake-weighted quorum math ✅
//! - DAG-S7: slashing (5–30% stake-weight per offense)
//!
//! Quorum: aggregate stake of Q_V exceeds (2/3) of total stake in V (Definition 2).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod registry;

pub use registry::{AdmissionError, ValidatorMember, ValidatorRegistry};

/// Validator Ring participant identifier.
pub type ValidatorId = u32;

/// Stake unit, in canonical GSX amounts. Sized for the 50,000-GSX × 500-
/// validator envelope without saturation.
pub type Stake = u128;

/// Genesis stake threshold per validator, in GSX.
pub const VALIDATOR_STAKE_THRESHOLD_GSX: u128 = 25_000;

/// Minimum Validator Ring size.
pub const VALIDATOR_RING_MIN: usize = 100;

/// Maximum Validator Ring size (target).
pub const VALIDATOR_RING_MAX: usize = 500;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_size_bounds_match_paper() {
        assert_eq!(VALIDATOR_RING_MIN, 100);
        assert_eq!(VALIDATOR_RING_MAX, 500);
    }

    #[test]
    fn stake_threshold_matches_paper() {
        assert_eq!(VALIDATOR_STAKE_THRESHOLD_GSX, 25_000);
    }
}
