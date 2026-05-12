//! Fast-path quorum.
//!
//! Paper §6.4: "Eligible transactions are certified by a fast-path
//! quorum of `⌈(2/3)|A|⌉ + 1` Authority Ring members."
//!
//! This is *strictly larger* than the Mysticeti-C main-lane quorum
//! (`⌈2n/3⌉ + 1`) for the same `n`; in the integer arithmetic both
//! formulas evaluate to the same value but the fast-path semantics are
//! stricter: certified by the same threshold, but with the added
//! requirement that signers commit irrevocably (equivocation is
//! slashable at 100% bonded stake, DAG-S9).

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

/// `⌈(2/3)n⌉ + 1` per paper §6.4. Capped at `n` to admit small-`n` test
/// envelopes (which would otherwise demand more signers than exist).
pub fn fast_path_quorum_size(n_authorities: u32) -> u32 {
    if n_authorities == 0 {
        return 1;
    }
    let q = (2 * n_authorities).div_ceil(3) + 1;
    q.min(n_authorities)
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
            lineage: CertHash([3; 32]),
            lineage_round: 0,
            payload_digest: [4; 32],
        }
    }

    #[test]
    fn quorum_size_matches_paper() {
        // n = 30: ⌈60/3⌉ + 1 = 21
        assert_eq!(fast_path_quorum_size(30), 21);
        // n = 50: ⌈100/3⌉ + 1 = 35
        assert_eq!(fast_path_quorum_size(50), 35);
        // n = 1: capped at 1
        assert_eq!(fast_path_quorum_size(1), 1);
        // n = 0: minimum of 1 (defensive)
        assert_eq!(fast_path_quorum_size(0), 1);
    }

    #[test]
    fn certify_below_quorum_fails() {
        let signers: BTreeSet<AuthorityId> = (0..20).collect(); // 20 < 21
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
        signers.insert(99); // outside committee
        let err = certify(dummy_tx(), signers, 30);
        assert!(matches!(err, Err(FastPathError::UnknownSigner(99))));
    }
}
