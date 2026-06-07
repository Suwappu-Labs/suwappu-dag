//! Authority Ring slashing pipeline.
//!
//! Per paper §5.1 and §6.4, Authority Node equivocation — whether on the
//! main-lane certificate DAG or on the fast-path lane — is slashable at
//! 100% of the offending node's bonded stake plus expulsion from the
//! ring. The slashing severity is identical on both lanes because the
//! integrity surface is the same: a single Authority signing two
//! distinct certificates at the same round breaks the certificate-DAG
//! uniqueness assumption that the Mysticeti-C commit rule relies on.
//!
//! The pipeline is deliberately atomic: a single `slash_authority` call
//! takes a verified equivocation proof and returns the slashing outcome.
//! Verification of the proof (signature checks, etc.) is performed by
//! the caller using the Authority's published `public_key_bytes`.

use crate::{AdmissionError, AuthorityId, AuthorityRegistry};

/// Outcome of a slashing event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthoritySlash {
    /// Stake forfeited (always equal to the member's bonded stake at
    /// slash time, since Authority slashing is 100%).
    pub stake_lost: u64,
    /// Whether the member was expelled from the ring.
    pub expelled: bool,
}

/// Slash an Authority for equivocation. Returns `Some(slash)` if the
/// member was seated and is now expelled, `None` if the member was
/// already absent (idempotent — repeat calls are safe).
pub fn slash_authority(
    registry: &mut AuthorityRegistry,
    author: AuthorityId,
) -> Option<AuthoritySlash> {
    let member = registry.remove(author)?;
    Some(AuthoritySlash {
        stake_lost: member.stake_suwappu,
        expelled: true,
    })
}

/// Re-admit a previously expelled Authority. Used only for emergency
/// recovery in Phase G3 governance (paper §14); not exercised on the
/// normal slashing path. Returns the same error variants as the
/// underlying [`AuthorityRegistry::admit`].
///
/// Re-admission requires a fresh stake post at or above the floor,
/// matching the original admission discipline.
pub fn readmit_authority(
    registry: &mut AuthorityRegistry,
    id: AuthorityId,
    stake_suwappu: u64,
    public_key_bytes: Vec<u8>,
) -> Result<(), AdmissionError> {
    registry.admit(crate::AuthorityMember {
        id,
        stake_suwappu,
        public_key_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AuthorityMember, AUTHORITY_STAKE_THRESHOLD_SUWAPPU};

    fn member(id: AuthorityId, stake: u64) -> AuthorityMember {
        AuthorityMember {
            id,
            stake_suwappu: stake,
            public_key_bytes: vec![id as u8; 32],
        }
    }

    #[test]
    fn slash_removes_member_and_returns_full_stake() {
        let mut r = AuthorityRegistry::new();
        r.admit(member(0, AUTHORITY_STAKE_THRESHOLD_SUWAPPU)).unwrap();
        let slash = slash_authority(&mut r, 0).unwrap();
        assert_eq!(slash.stake_lost, AUTHORITY_STAKE_THRESHOLD_SUWAPPU);
        assert!(slash.expelled);
        assert!(!r.contains(0));
    }

    #[test]
    fn slash_is_idempotent() {
        let mut r = AuthorityRegistry::new();
        r.admit(member(0, AUTHORITY_STAKE_THRESHOLD_SUWAPPU)).unwrap();
        slash_authority(&mut r, 0).unwrap();
        let again = slash_authority(&mut r, 0);
        assert!(again.is_none());
    }

    #[test]
    fn slash_absent_member_returns_none() {
        let mut r = AuthorityRegistry::new();
        assert!(slash_authority(&mut r, 42).is_none());
    }

    #[test]
    fn readmit_after_slash_requires_fresh_stake() {
        let mut r = AuthorityRegistry::new();
        r.admit(member(0, AUTHORITY_STAKE_THRESHOLD_SUWAPPU)).unwrap();
        slash_authority(&mut r, 0).unwrap();
        readmit_authority(&mut r, 0, AUTHORITY_STAKE_THRESHOLD_SUWAPPU, vec![0; 32]).unwrap();
        assert!(r.contains(0));
    }
}
