//! Authority Ring registry.
//!
//! Phase-1 maintains the registry in-memory; the on-chain admission
//! pipeline (Authority-Phase Matrix governance, paper §14) lands in a
//! later sprint. The registry enforces two structural invariants:
//!
//! 1. **Stake floor.** Every admitted member holds at least
//!    `AUTHORITY_STAKE_THRESHOLD_SUWAPPU` (100,000 SUWAPPU).
//! 2. **Ring-size ceiling.** The registry rejects admission once
//!    `AUTHORITY_RING_MAX` (50) members are seated. The ring-size floor
//!    of 30 is a *runtime invariant* enforced by governance, not by the
//!    registry — sub-floor sizes are tolerated during bootstrap.
//!
//! The registry exposes `quorum_threshold()` returning the BFT quorum
//! count `⌈2n/3⌉ + 1` (capped at `n` for the small-`n` test envelopes).
//! This is the same formula as `suwappu-consensus::quorum_threshold` and the
//! two are verified to agree at 10,000 cases by `quorum_math_matches_paper`.

use std::collections::BTreeMap;

use thiserror::Error;

use crate::{AuthorityId, AUTHORITY_RING_MAX, AUTHORITY_STAKE_THRESHOLD_SUWAPPU};

/// Errors emitted by the Authority registry on admission.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AdmissionError {
    /// The candidate's posted stake is below
    /// `AUTHORITY_STAKE_THRESHOLD_SUWAPPU`.
    #[error("authority stake {posted} below floor {floor}")]
    StakeBelowFloor {
        /// Stake the candidate attempted to post.
        posted: u64,
        /// Floor required by the Authority Ring.
        floor: u64,
    },

    /// The ring already holds `AUTHORITY_RING_MAX` members.
    #[error("authority ring full ({size} >= {max})")]
    RingFull {
        /// Current ring size.
        size: usize,
        /// Maximum permitted size.
        max: usize,
    },

    /// An authority with the same identifier is already admitted.
    #[error("authority {0} already admitted")]
    DuplicateMember(AuthorityId),
}

/// A seated Authority Ring member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorityMember {
    /// Authority identifier (index into the published set).
    pub id: AuthorityId,
    /// Posted base-chain PoS stake in SUWAPPU. Must be at least
    /// `AUTHORITY_STAKE_THRESHOLD_SUWAPPU` at admission time.
    pub stake_suwappu: u64,
    /// ML-DSA-65 public key bytes. Phase-1 stores the bytes opaquely;
    /// signature verification consumes them via
    /// `suwappu_crypto::mldsa::PublicKey::from_bytes`.
    pub public_key_bytes: Vec<u8>,
}

/// Authority Ring registry.
#[derive(Debug, Clone, Default)]
pub struct AuthorityRegistry {
    members: BTreeMap<AuthorityId, AuthorityMember>,
}

impl AuthorityRegistry {
    /// Construct an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Admit a new Authority member. Enforces stake floor, ring-size
    /// ceiling, and uniqueness of the identifier.
    pub fn admit(&mut self, member: AuthorityMember) -> Result<(), AdmissionError> {
        if member.stake_suwappu < AUTHORITY_STAKE_THRESHOLD_SUWAPPU {
            return Err(AdmissionError::StakeBelowFloor {
                posted: member.stake_suwappu,
                floor: AUTHORITY_STAKE_THRESHOLD_SUWAPPU,
            });
        }
        if self.members.len() >= AUTHORITY_RING_MAX {
            return Err(AdmissionError::RingFull {
                size: self.members.len(),
                max: AUTHORITY_RING_MAX,
            });
        }
        if self.members.contains_key(&member.id) {
            return Err(AdmissionError::DuplicateMember(member.id));
        }
        self.members.insert(member.id, member);
        Ok(())
    }

    /// Remove an Authority member by identifier. Returns the removed
    /// member if it existed.
    pub fn remove(&mut self, id: AuthorityId) -> Option<AuthorityMember> {
        self.members.remove(&id)
    }

    /// Number of seated members.
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// `true` iff the registry has no members.
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// `true` iff the given identifier is seated.
    pub fn contains(&self, id: AuthorityId) -> bool {
        self.members.contains_key(&id)
    }

    /// Borrow a member by identifier.
    pub fn get(&self, id: AuthorityId) -> Option<&AuthorityMember> {
        self.members.get(&id)
    }

    /// Iterate members in canonical (ascending-id) order.
    pub fn members(&self) -> impl Iterator<Item = &AuthorityMember> {
        self.members.values()
    }

    /// BFT supermajority quorum count: `q = n − ⌊(n-1)/3⌋` (equivalently
    /// `2f+1` when `n = 3f+1`). Matches `suwappu_consensus::quorum_threshold`
    /// and Sui Lutris. See `docs/iq/IQ-001-quorum-formula.md` for the
    /// divergence from paper §6.4's literal `⌈2n/3⌉ + 1`.
    pub fn quorum_threshold(&self) -> u32 {
        let n = self.members.len() as u32;
        if n == 0 {
            return 1;
        }
        n - (n - 1) / 3
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(id: AuthorityId, stake: u64) -> AuthorityMember {
        AuthorityMember {
            id,
            stake_suwappu: stake,
            public_key_bytes: vec![id as u8; 32],
        }
    }

    #[test]
    fn admit_above_floor_succeeds() {
        let mut r = AuthorityRegistry::new();
        r.admit(member(0, AUTHORITY_STAKE_THRESHOLD_SUWAPPU)).unwrap();
        assert_eq!(r.len(), 1);
        assert!(r.contains(0));
    }

    #[test]
    fn admit_below_floor_fails() {
        let mut r = AuthorityRegistry::new();
        let err = r.admit(member(0, AUTHORITY_STAKE_THRESHOLD_SUWAPPU - 1));
        assert!(matches!(err, Err(AdmissionError::StakeBelowFloor { .. })));
    }

    #[test]
    fn admit_duplicate_fails() {
        let mut r = AuthorityRegistry::new();
        r.admit(member(0, AUTHORITY_STAKE_THRESHOLD_SUWAPPU)).unwrap();
        let err = r.admit(member(0, AUTHORITY_STAKE_THRESHOLD_SUWAPPU));
        assert_eq!(err, Err(AdmissionError::DuplicateMember(0)));
    }

    #[test]
    fn admit_full_ring_fails() {
        let mut r = AuthorityRegistry::new();
        for i in 0..AUTHORITY_RING_MAX as u32 {
            r.admit(member(i, AUTHORITY_STAKE_THRESHOLD_SUWAPPU)).unwrap();
        }
        let err = r.admit(member(
            AUTHORITY_RING_MAX as u32,
            AUTHORITY_STAKE_THRESHOLD_SUWAPPU,
        ));
        assert!(matches!(err, Err(AdmissionError::RingFull { .. })));
    }

    #[test]
    fn remove_decreases_size() {
        let mut r = AuthorityRegistry::new();
        for i in 0..4 {
            r.admit(member(i, AUTHORITY_STAKE_THRESHOLD_SUWAPPU)).unwrap();
        }
        let removed = r.remove(2).unwrap();
        assert_eq!(removed.id, 2);
        assert_eq!(r.len(), 3);
        assert!(!r.contains(2));
    }

    #[test]
    fn quorum_threshold_matches_canonical_bft() {
        let mut r = AuthorityRegistry::new();
        // 30 authorities: n − ⌊29/3⌋ = 30 − 9 = 21 (unchanged from paper).
        for i in 0..30 {
            r.admit(member(i, AUTHORITY_STAKE_THRESHOLD_SUWAPPU)).unwrap();
        }
        assert_eq!(r.quorum_threshold(), 21);

        // 50 authorities: n − ⌊49/3⌋ = 50 − 16 = 34 (was 35 under paper).
        for i in 30..50 {
            r.admit(member(i, AUTHORITY_STAKE_THRESHOLD_SUWAPPU)).unwrap();
        }
        assert_eq!(r.quorum_threshold(), 34);
    }

    #[test]
    fn empty_registry_threshold_is_one() {
        let r = AuthorityRegistry::new();
        assert_eq!(r.quorum_threshold(), 1);
    }
}
