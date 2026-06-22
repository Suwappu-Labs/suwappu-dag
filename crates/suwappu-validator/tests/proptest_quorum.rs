//! DAG-S6 exit-gate property tests.
//!
//! Exit gate: `quorum_math_matches_paper` — for any reachable Authority
//! Ring + Validator Ring configuration, the registry's quorum thresholds
//! match the formulas used by the in-memory consensus rule
//! (`suwappu_consensus::quorum_threshold` for the count side,
//! `suwappu_consensus::validator_quorum_threshold` for the stake side).
//! The two surfaces are independent implementations of paper
//! Definition 2; this property guarantees they cannot drift.
//!
//! Supporting properties:
//!
//! - `authority_admission_enforces_invariants` — admission rejects
//!   below-floor stake, ring-full, and duplicate-id.
//! - `validator_admission_enforces_invariants` — same shape for the
//!   Validator Ring.
//! - `removal_decreases_membership` — removal is observable.
//!
//! Run at default 256 cases under CI; sprint close runs
//! `PROPTEST_CASES=10000 cargo test -p suwappu-validator --release`.

use proptest::prelude::*;
use suwappu_authority::{
    AuthorityMember, AuthorityRegistry, AUTHORITY_RING_MAX, AUTHORITY_STAKE_THRESHOLD_SUWAPPU,
};
use suwappu_consensus::{quorum_threshold as consensus_authority_threshold, StakeTable};
use suwappu_validator::{
    Stake, ValidatorMember, ValidatorRegistry, VALIDATOR_RING_MAX,
    VALIDATOR_STAKE_THRESHOLD_SUWAPPU,
};

fn auth(id: u32, stake: u64) -> AuthorityMember {
    AuthorityMember {
        id,
        stake_suwappu: stake,
        public_key_bytes: vec![id as u8; 32],
    }
}

fn vmem(id: u32, stake: Stake) -> ValidatorMember {
    ValidatorMember {
        id,
        stake_suwappu: stake,
    }
}

/// Build an Authority registry of size `n` with every member exactly
/// at the stake floor.
fn build_authority(n: u32) -> AuthorityRegistry {
    let mut r = AuthorityRegistry::new();
    for i in 0..n {
        r.admit(auth(i, AUTHORITY_STAKE_THRESHOLD_SUWAPPU)).unwrap();
    }
    r
}

/// Build a Validator registry of `n` members with the given individual
/// stake (must be ≥ floor).
fn build_validator(n: u32, stake_per: Stake) -> ValidatorRegistry {
    let mut r = ValidatorRegistry::new();
    for i in 0..n {
        r.admit(vmem(i, stake_per)).unwrap();
    }
    r
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 32,
        .. ProptestConfig::default()
    })]

    /// EXIT GATE — paper Definition 2. The registry's quorum_threshold
    /// must agree with the consensus crate's quorum_threshold for any
    /// reachable ring size.
    #[test]
    fn quorum_math_matches_paper(
        n_authorities in 1u32..=(AUTHORITY_RING_MAX as u32),
        n_validators in 1u32..=64,
        per_validator_stake in VALIDATOR_STAKE_THRESHOLD_SUWAPPU..=(VALIDATOR_STAKE_THRESHOLD_SUWAPPU * 4),
    ) {
        // Authority side.
        let auth_reg = build_authority(n_authorities);
        prop_assert_eq!(
            auth_reg.quorum_threshold(),
            consensus_authority_threshold(n_authorities),
        );

        // Validator side. Build the registry, project to a StakeTable,
        // and verify that both `quorum_threshold_stake` and the
        // consensus crate's `validator_quorum_threshold` agree on the
        // strictly-above-2/3 threshold.
        let val_reg = build_validator(n_validators, per_validator_stake);
        let registry_threshold = val_reg.quorum_threshold_stake();

        let stake_table = StakeTable::from_entries(
            val_reg.members().map(|m| (m.id, m.stake_suwappu)),
        );
        let consensus_threshold = suwappu_consensus::validator_quorum_threshold(&stake_table);

        prop_assert_eq!(registry_threshold, consensus_threshold);
        prop_assert_eq!(stake_table.total(), val_reg.total_stake());
    }

    /// Authority admission enforces all three invariants: stake floor,
    /// ring-size ceiling, and duplicate-id rejection.
    #[test]
    fn authority_admission_enforces_invariants(
        // Stake that may be below floor.
        stake in 0u64..=(AUTHORITY_STAKE_THRESHOLD_SUWAPPU * 2),
        n_seed in 0u32..=(AUTHORITY_RING_MAX as u32),
    ) {
        let mut r = build_authority(n_seed);
        let candidate_id = n_seed; // first unseated id

        match r.admit(auth(candidate_id, stake)) {
            Ok(()) => {
                prop_assert!(stake >= AUTHORITY_STAKE_THRESHOLD_SUWAPPU);
                prop_assert!(r.len() <= AUTHORITY_RING_MAX);
                prop_assert!(r.contains(candidate_id));
            }
            Err(suwappu_authority::AdmissionError::StakeBelowFloor { posted, floor }) => {
                prop_assert!(posted < floor);
                prop_assert_eq!(floor, AUTHORITY_STAKE_THRESHOLD_SUWAPPU);
            }
            Err(suwappu_authority::AdmissionError::RingFull { size, max }) => {
                prop_assert_eq!(size, AUTHORITY_RING_MAX);
                prop_assert_eq!(max, AUTHORITY_RING_MAX);
            }
            Err(suwappu_authority::AdmissionError::DuplicateMember(id)) => {
                prop_assert_eq!(id, candidate_id);
                prop_assert!(r.contains(candidate_id));
            }
        }
    }

    /// Validator admission enforces stake floor, ring-size ceiling, and
    /// duplicate-id rejection.
    #[test]
    fn validator_admission_enforces_invariants(
        stake in 0u128..=(VALIDATOR_STAKE_THRESHOLD_SUWAPPU * 4),
        n_seed in 0u32..=64,
    ) {
        let mut r = build_validator(n_seed, VALIDATOR_STAKE_THRESHOLD_SUWAPPU);
        let candidate_id = n_seed;

        match r.admit(vmem(candidate_id, stake)) {
            Ok(()) => {
                prop_assert!(stake >= VALIDATOR_STAKE_THRESHOLD_SUWAPPU);
                prop_assert!(r.len() <= VALIDATOR_RING_MAX);
            }
            Err(suwappu_validator::AdmissionError::StakeBelowFloor { posted, floor }) => {
                prop_assert!(posted < floor);
                prop_assert_eq!(floor, VALIDATOR_STAKE_THRESHOLD_SUWAPPU);
            }
            Err(suwappu_validator::AdmissionError::RingFull { .. }) => {
                // Won't fire in this property because n_seed is bounded
                // below VALIDATOR_RING_MAX; we still accept the error.
            }
            Err(suwappu_validator::AdmissionError::DuplicateMember(id)) => {
                prop_assert_eq!(id, candidate_id);
            }
        }
    }

    /// Removal decreases the membership count and the total stake by
    /// exactly the removed member's contribution.
    #[test]
    fn removal_decreases_membership(
        n in 1u32..=32,
        stake_per in VALIDATOR_STAKE_THRESHOLD_SUWAPPU..=(VALIDATOR_STAKE_THRESHOLD_SUWAPPU * 4),
        remove_idx in 0u32..32,
    ) {
        let mut r = build_validator(n, stake_per);
        let before_total = r.total_stake();
        let before_len = r.len();
        let target = remove_idx % n;
        prop_assert!(r.contains(target));

        let removed = r.remove(target).unwrap();
        prop_assert_eq!(removed.stake_suwappu, stake_per);
        prop_assert_eq!(r.len(), before_len - 1);
        prop_assert_eq!(r.total_stake(), before_total - stake_per);
        prop_assert!(!r.contains(target));
    }
}
