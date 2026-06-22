//! Validator Ring registry.
//!
//! Phase-1 maintains the registry in-memory; the on-chain validator-set
//! contract lands later. The registry enforces:
//!
//! 1. **Stake floor.** Every admitted validator holds at least
//!    `VALIDATOR_STAKE_THRESHOLD_SUWAPPU` (25,000 SUWAPPU).
//! 2. **Ring-size ceiling.** The registry rejects admission once
//!    `VALIDATOR_RING_MAX` (500) validators are seated. The ring-size
//!    floor of 100 is a runtime invariant enforced by governance, not
//!    by the registry — sub-floor sizes are tolerated during bootstrap.
//!
//! The registry exposes `quorum_threshold_stake()` returning the BFT
//! stake-weighted quorum, strictly greater than two-thirds of total
//! stake. This is the same formula as
//! `suwappu-consensus::validator_quorum_threshold` and the two are verified
//! to agree at 10,000 cases by `validator_quorum_math_matches_paper`.

use std::collections::BTreeMap;

use thiserror::Error;

use crate::{Stake, ValidatorId, VALIDATOR_RING_MAX, VALIDATOR_STAKE_THRESHOLD_SUWAPPU};

/// Errors emitted by the Validator registry on admission.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AdmissionError {
    /// The candidate's posted stake is below
    /// `VALIDATOR_STAKE_THRESHOLD_SUWAPPU`.
    #[error("validator stake {posted} below floor {floor}")]
    StakeBelowFloor {
        /// Stake the candidate attempted to post.
        posted: Stake,
        /// Floor required by the Validator Ring.
        floor: Stake,
    },

    /// The ring already holds `VALIDATOR_RING_MAX` members.
    #[error("validator ring full ({size} >= {max})")]
    RingFull {
        /// Current ring size.
        size: usize,
        /// Maximum permitted size.
        max: usize,
    },

    /// A validator with the same identifier is already admitted.
    #[error("validator {0} already admitted")]
    DuplicateMember(ValidatorId),
}

/// A seated Validator Ring member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatorMember {
    /// Validator identifier.
    pub id: ValidatorId,
    /// Posted stake in SUWAPPU. Must be at least
    /// `VALIDATOR_STAKE_THRESHOLD_SUWAPPU` at admission time.
    pub stake_suwappu: Stake,
}

/// Validator Ring registry.
#[derive(Debug, Clone, Default)]
pub struct ValidatorRegistry {
    members: BTreeMap<ValidatorId, ValidatorMember>,
}

impl ValidatorRegistry {
    /// Construct an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Admit a new Validator member.
    pub fn admit(&mut self, member: ValidatorMember) -> Result<(), AdmissionError> {
        if member.stake_suwappu < VALIDATOR_STAKE_THRESHOLD_SUWAPPU {
            return Err(AdmissionError::StakeBelowFloor {
                posted: member.stake_suwappu,
                floor: VALIDATOR_STAKE_THRESHOLD_SUWAPPU,
            });
        }
        if self.members.len() >= VALIDATOR_RING_MAX {
            return Err(AdmissionError::RingFull {
                size: self.members.len(),
                max: VALIDATOR_RING_MAX,
            });
        }
        if self.members.contains_key(&member.id) {
            return Err(AdmissionError::DuplicateMember(member.id));
        }
        self.members.insert(member.id, member);
        Ok(())
    }

    /// Remove a validator by id. Returns the removed member if it existed.
    pub fn remove(&mut self, id: ValidatorId) -> Option<ValidatorMember> {
        self.members.remove(&id)
    }

    /// Number of seated validators.
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// `true` iff no validators are seated.
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// `true` iff the identifier is seated.
    pub fn contains(&self, id: ValidatorId) -> bool {
        self.members.contains_key(&id)
    }

    /// Borrow the seated validator with the given identifier.
    pub fn get(&self, id: ValidatorId) -> Option<&ValidatorMember> {
        self.members.get(&id)
    }

    /// Iterate validators in canonical (ascending-id) order.
    pub fn members(&self) -> impl Iterator<Item = &ValidatorMember> {
        self.members.values()
    }

    /// Total stake across the ring.
    pub fn total_stake(&self) -> Stake {
        self.members.values().map(|m| m.stake_suwappu).sum()
    }

    /// Stake-weighted BFT quorum threshold: strictly greater than
    /// two-thirds of total stake. Matches paper Definition 2 and the
    /// `suwappu_consensus::validator_quorum_threshold` formula used by the
    /// joint-quorum AND-gate.
    pub fn quorum_threshold_stake(&self) -> Stake {
        let total = self.total_stake();
        (2 * total) / 3 + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(id: ValidatorId, stake: Stake) -> ValidatorMember {
        ValidatorMember {
            id,
            stake_suwappu: stake,
        }
    }

    #[test]
    fn admit_above_floor_succeeds() {
        let mut r = ValidatorRegistry::new();
        r.admit(member(0, VALIDATOR_STAKE_THRESHOLD_SUWAPPU))
            .unwrap();
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn admit_below_floor_fails() {
        let mut r = ValidatorRegistry::new();
        let err = r.admit(member(0, VALIDATOR_STAKE_THRESHOLD_SUWAPPU - 1));
        assert!(matches!(err, Err(AdmissionError::StakeBelowFloor { .. })));
    }

    #[test]
    fn admit_duplicate_fails() {
        let mut r = ValidatorRegistry::new();
        r.admit(member(0, VALIDATOR_STAKE_THRESHOLD_SUWAPPU))
            .unwrap();
        assert_eq!(
            r.admit(member(0, VALIDATOR_STAKE_THRESHOLD_SUWAPPU)),
            Err(AdmissionError::DuplicateMember(0)),
        );
    }

    #[test]
    fn admit_full_ring_fails() {
        let mut r = ValidatorRegistry::new();
        for i in 0..VALIDATOR_RING_MAX as u32 {
            r.admit(member(i, VALIDATOR_STAKE_THRESHOLD_SUWAPPU))
                .unwrap();
        }
        let err = r.admit(member(
            VALIDATOR_RING_MAX as u32,
            VALIDATOR_STAKE_THRESHOLD_SUWAPPU,
        ));
        assert!(matches!(err, Err(AdmissionError::RingFull { .. })));
    }

    #[test]
    fn total_stake_sums_members() {
        let mut r = ValidatorRegistry::new();
        for i in 0..5 {
            r.admit(member(i, VALIDATOR_STAKE_THRESHOLD_SUWAPPU))
                .unwrap();
        }
        assert_eq!(r.total_stake(), 5 * VALIDATOR_STAKE_THRESHOLD_SUWAPPU);
    }

    #[test]
    fn quorum_threshold_strictly_above_two_thirds() {
        let mut r = ValidatorRegistry::new();
        // 9 validators each at the floor (25,000 SUWAPPU). Total stake
        // 225,000; 2/3 = 150,000; threshold > 150,000 → 150,001.
        for i in 0..9 {
            r.admit(member(i, VALIDATOR_STAKE_THRESHOLD_SUWAPPU))
                .unwrap();
        }
        assert_eq!(r.quorum_threshold_stake(), 150_001);
    }
}
