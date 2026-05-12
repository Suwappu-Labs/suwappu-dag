//! Fast-path slashing pipeline.
//!
//! Paper §6.4: every Authority Node listed in `cert.signers` of a
//! fast-path equivocation proof is slashed at 100% of bonded stake plus
//! expulsion. This is the same severity as main-lane Authority
//! equivocation (DAG-S7) and is implemented by reusing the
//! `gsx_authority::slash_authority` primitive for each signer.

use gsx_authority::{slash_authority, AuthorityRegistry, AuthoritySlash};
use gsx_consensus::AuthorityId;

use crate::equivocation::FastPathEquivocationProof;

/// Outcome of slashing every signer in a fast-path equivocation proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FastPathSlashOutcome {
    /// One entry per signer that was actually slashed (was seated in
    /// the registry at slash time). Signers absent at slash time are
    /// recorded in `missing`.
    pub slashed: Vec<(AuthorityId, AuthoritySlash)>,
    /// Signers from `cert.signers` who were not in the registry (already
    /// expelled or never admitted). No-op for those.
    pub missing: Vec<AuthorityId>,
}

impl FastPathSlashOutcome {
    /// Total stake forfeited across all slashed signers.
    pub fn total_stake_lost(&self) -> u64 {
        self.slashed.iter().map(|(_, s)| s.stake_lost).sum()
    }
}

/// Slash every signer in `proof.cert.signers` at 100% bonded stake plus
/// expulsion. Signers absent at slash time are reported in `missing`
/// and do not cause an error — the pipeline is idempotent.
pub fn slash_fast_path_signers(
    registry: &mut AuthorityRegistry,
    proof: &FastPathEquivocationProof,
) -> FastPathSlashOutcome {
    let mut slashed = Vec::new();
    let mut missing = Vec::new();
    for &author in &proof.cert.signers {
        match slash_authority(registry, author) {
            Some(s) => slashed.push((author, s)),
            None => missing.push(author),
        }
    }
    FastPathSlashOutcome { slashed, missing }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use gsx_authority::{AuthorityMember, AUTHORITY_STAKE_THRESHOLD_GSX};
    use gsx_consensus::CertHash;

    use super::*;
    use crate::{
        binding::MainLaneTx,
        cert::{FastPathCert, FastPathTx, OwnedObjectId, OwnerAddress},
    };

    fn seat_authorities(registry: &mut AuthorityRegistry, n: u32) {
        for i in 0..n {
            registry
                .admit(AuthorityMember {
                    id: i,
                    stake_gsx: AUTHORITY_STAKE_THRESHOLD_GSX,
                    public_key_bytes: vec![i as u8; 32],
                })
                .unwrap();
        }
    }

    fn proof_with_signers(signers: BTreeSet<AuthorityId>) -> FastPathEquivocationProof {
        let obj = OwnedObjectId([1; 32]);
        FastPathEquivocationProof {
            cert: FastPathCert {
                tx: FastPathTx {
                    object: obj,
                    owner: OwnerAddress([0; 32]),
                    nonce: 0,
                    lineage: CertHash([0; 32]),
                    lineage_round: 10,
                    payload_digest: [0xA; 32],
                },
                signers,
            },
            conflicting_tx: MainLaneTx {
                round: 12,
                object: obj,
                payload_digest: [0xB; 32],
                lineage: CertHash([0; 32]),
            },
        }
    }

    #[test]
    fn every_seated_signer_is_expelled() {
        let mut registry = AuthorityRegistry::new();
        seat_authorities(&mut registry, 4);

        let signers: BTreeSet<AuthorityId> = BTreeSet::from([0, 1, 2, 3]);
        let proof = proof_with_signers(signers.clone());
        let outcome = slash_fast_path_signers(&mut registry, &proof);

        assert_eq!(outcome.slashed.len(), 4);
        assert!(outcome.missing.is_empty());
        assert_eq!(
            outcome.total_stake_lost(),
            4 * AUTHORITY_STAKE_THRESHOLD_GSX,
        );
        for author in signers {
            assert!(!registry.contains(author));
        }
    }

    #[test]
    fn unseated_signers_recorded_as_missing() {
        let mut registry = AuthorityRegistry::new();
        seat_authorities(&mut registry, 2);

        let signers: BTreeSet<AuthorityId> = BTreeSet::from([0, 1, 99]);
        let proof = proof_with_signers(signers);
        let outcome = slash_fast_path_signers(&mut registry, &proof);

        assert_eq!(outcome.slashed.len(), 2);
        assert_eq!(outcome.missing, vec![99]);
    }

    #[test]
    fn slashing_is_idempotent() {
        let mut registry = AuthorityRegistry::new();
        seat_authorities(&mut registry, 4);
        let proof = proof_with_signers(BTreeSet::from([0, 1, 2, 3]));

        let first = slash_fast_path_signers(&mut registry, &proof);
        assert_eq!(first.slashed.len(), 4);

        let second = slash_fast_path_signers(&mut registry, &proof);
        assert!(second.slashed.is_empty());
        assert_eq!(second.missing.len(), 4);
    }

    #[test]
    fn unrelated_authorities_are_not_affected() {
        let mut registry = AuthorityRegistry::new();
        seat_authorities(&mut registry, 5);

        let proof = proof_with_signers(BTreeSet::from([0, 1, 2]));
        slash_fast_path_signers(&mut registry, &proof);

        // Authors 3, 4 should still be seated.
        assert!(registry.contains(3));
        assert!(registry.contains(4));
    }
}
