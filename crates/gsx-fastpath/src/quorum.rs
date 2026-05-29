//! Fast-path quorum.
//!
//! Paper §6.4 writes `⌈(2/3)|A|⌉ + 1`, but per `docs/iq/IQ-001` the
//! canonical Mysticeti / Bullshark / Sui threshold is `q = 2f + 1` with
//! `f = ⌊(n-1)/3⌋` — equivalently `q = n − ⌊(n-1)/3⌋`. This matches
//! the main-lane `gsx_consensus::commit::quorum_threshold`. Fast-path
//! semantics remain stricter than the main lane only in that signers
//! commit irrevocably: equivocation is slashable at 100% bonded stake
//! (DAG-S9).

use std::collections::BTreeSet;

use gsx_consensus::AuthorityId;
use thiserror::Error;

use crate::cert::{FastPathCert, FastPathTx};

/// Errors produced by the fast-path quorum surface.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FastPathError {
    /// Supplied signer set is below the fast-path threshold.
    #[error("fast-path quorum below threshold: have {have}, need {need}")]
    BelowQuorum {
        /// Number of signers supplied.
        have: u32,
        /// Number of signers required.
        need: u32,
    },

    /// Signer set referenced a non-existent committee member.
    #[error("fast-path signer {0} outside committee bounds")]
    UnknownSigner(AuthorityId),
}

/// `q = 2f + 1` with `f = ⌊(n-1)/3⌋` — equivalently `n − ⌊(n-1)/3⌋`.
/// Matches `gsx_consensus::commit::quorum_threshold`. See IQ-001 for the
/// divergence from paper §6.4's literal `⌈2n/3⌉ + 1`.
pub fn fast_path_quorum_size(n_authorities: u32) -> u32 {
    if n_authorities == 0 {
        return 1;
    }
    n_authorities - (n_authorities - 1) / 3
}

/// Verify that `signers` constitutes a fast-path quorum for the supplied
/// committee size, and produce a [`FastPathCert`].
pub fn certify(
    tx: FastPathTx,
    signers: BTreeSet<AuthorityId>,
    n_authorities: u32,
) -> Result<FastPathCert, FastPathError> {
    // Every signer must be within committee bounds.
    for &s in &signers {
        if s >= n_authorities {
            return Err(FastPathError::UnknownSigner(s));
        }
    }
    let need = fast_path_quorum_size(n_authorities);
    let have = signers.len() as u32;
    if have < need {
        return Err(FastPathError::BelowQuorum { have, need });
    }
    Ok(FastPathCert { tx, signers })
}

#[cfg(test)]
mod tests {
    use gsx_consensus::CertHash;

    use super::*;
    use crate::cert::{OwnedObjectId, OwnerAddress};

    fn dummy_tx() -> FastPathTx {
        FastPathTx {
            object: OwnedObjectId([1; 32]),
            owner: OwnerAddress([2; 32]),
            nonce: 0,
            lineage: CertHash::from([3; 32]),
            lineage_round: 0,
            payload_digest: [4; 32],
        }
    }

    #[test]
    fn quorum_size_matches_canonical_bft() {
        // q = n − ⌊(n-1)/3⌋ — equivalent to ⌊2n/3⌋ + 1 and to 2f+1 when
        // n = 3f+1. See IQ-001 for divergence from paper §6.4's `⌈2n/3⌉+1`.
        assert_eq!(fast_path_quorum_size(0), 1); // defensive
        assert_eq!(fast_path_quorum_size(1), 1);
        assert_eq!(fast_path_quorum_size(4), 3); // was 4 under paper (unanimity bug)
        assert_eq!(fast_path_quorum_size(7), 5); // was 6 under paper
        assert_eq!(fast_path_quorum_size(10), 7); // was 8 under paper
        assert_eq!(fast_path_quorum_size(30), 21);
        assert_eq!(fast_path_quorum_size(50), 34); // was 35 under paper

        // Property: q > 2n/3 (strict supermajority) for every committee size.
        for n in 1u32..=64 {
            let q = fast_path_quorum_size(n);
            assert!(3 * q > 2 * n, "n={n}: q={q} fails strict 2/3 majority");
            assert!(q <= n, "n={n}: q={q} exceeds committee");
        }
    }

    #[test]
    fn certify_below_quorum_fails() {
        // n=30, q=21. 20 signers is below.
        let signers: BTreeSet<AuthorityId> = (0..20).collect();
        let err = certify(dummy_tx(), signers, 30);
        assert!(matches!(err, Err(FastPathError::BelowQuorum { .. })));
    }

    #[test]
    fn certify_at_quorum_succeeds() {
        let signers: BTreeSet<AuthorityId> = (0..21).collect();
        let cert = certify(dummy_tx(), signers, 30).unwrap();
        assert_eq!(cert.signers.len(), 21);
    }

    #[test]
    fn certify_with_out_of_bounds_signer_fails() {
        let mut signers: BTreeSet<AuthorityId> = (0..21).collect();
        signers.insert(99);
        let err = certify(dummy_tx(), signers, 30);
        assert!(matches!(err, Err(FastPathError::UnknownSigner(99))));
    }
}
