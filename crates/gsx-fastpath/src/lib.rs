//! gsx-fastpath — FastPay-style fast-path lane.
//!
//! Implements §6.4 of the *GSX DAG Layer 1* paper:
//!
//! - Parallel to main-lane Mysticeti consensus
//! - Eligible: read-write footprint is a single owned Move object, owner is sole
//!   signer, lineage grounded in a main-lane path
//! - Quorum: ⌈(2/3)|A|⌉ + 1 Authority Ring members
//! - 100–200 ms p95 finality
//! - Equivocation: slashable at 100% Authority-Node bonded stake + expulsion
//!
//! Sprint scope:
//!
//! - DAG-S8: eligibility check, fast-path certificate type, K=4 main-lane
//!   confirmation rounds
//! - DAG-S9: equivocation detection and slashing path
//!
//! Property: a fast-path-certified transaction whose main-lane confirmation
//! observes a conflicting ordering yields an equivocation proof that slashes
//! every signing Authority Node at 100% of bonded stake.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Main-lane confirmation depth K for fast-path binding (paper §6.4).
pub const FAST_PATH_CONFIRMATION_K: u32 = 4;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_k_matches_paper() {
        assert_eq!(FAST_PATH_CONFIRMATION_K, 4);
    }
}
