//! Fast-path equivocation detection.
//!
//! Paper §6.4: "equivocation is slashable at 100% of the offending
//! Authority Node's bonded stake plus expulsion." The fast-path
//! equivocation signal is generated when a fast-path certificate is
//! observed against a main-lane confirmation that contradicts the
//! certified payload within the binding window `(R, R+K]`.
//!
//! The proof carries:
//!
//! - The certificate itself, naming the signers who attested.
//! - The conflicting `MainLaneTx` (same object, different payload digest,
//!   within window).
//!
//! Downstream slashing (`slashing::slash_fast_path_signers`) hits every
//! signer in `cert.signers` at 100% + expulsion.

use serde::{Deserialize, Serialize};

use crate::{
    binding::{MainLaneTx, FAST_PATH_CONFIRMATION_K},
    cert::FastPathCert,
};

/// Cryptographically-verifiable proof that the signers of `cert`
/// equivocated, in the form of a conflicting main-lane transaction
/// inside the binding window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FastPathEquivocationProof {
    /// The fast-path certificate whose signers are being accused.
    pub cert: FastPathCert,
    /// A main-lane tx that conflicts with the certified payload inside
    /// `(R, R+K]`.
    pub conflicting_tx: MainLaneTx,
}

/// Scan `main_lane` and return the first conflicting main-lane tx
/// inside the binding window, if any.
///
/// Returns `Some(proof)` iff a tx exists in `(lineage_round, lineage_round + K]`
/// that touches `cert.tx.object` with a different `payload_digest`.
pub fn detect_fast_path_equivocation(
    cert: &FastPathCert,
    main_lane: &[MainLaneTx],
) -> Option<FastPathEquivocationProof> {
    let lo = cert.tx.lineage_round + 1;
    let hi = cert.tx.lineage_round + FAST_PATH_CONFIRMATION_K as u64;
    for ml_tx in main_lane {
        if ml_tx.round < lo || ml_tx.round > hi {
            continue;
        }
        if ml_tx.object == cert.tx.object && ml_tx.payload_digest != cert.tx.payload_digest {
            return Some(FastPathEquivocationProof {
                cert: cert.clone(),
                conflicting_tx: *ml_tx,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use suwappu_consensus::CertHash;

    use super::*;
    use crate::cert::{FastPathTx, OwnedObjectId, OwnerAddress};

    fn cert_at(round: u64, object: OwnedObjectId, payload: [u8; 32]) -> FastPathCert {
        FastPathCert {
            tx: FastPathTx {
                object,
                owner: OwnerAddress([0; 32]),
                nonce: 0,
                lineage: CertHash([0; 32]),
                lineage_round: round,
                payload_digest: payload,
            },
            signers: BTreeSet::from([0, 1, 2, 3]),
        }
    }

    #[test]
    fn no_conflict_yields_no_proof() {
        let cert = cert_at(0, OwnedObjectId([1; 32]), [0xA; 32]);
        assert!(detect_fast_path_equivocation(&cert, &[]).is_none());
    }

    #[test]
    fn conflict_inside_window_yields_proof() {
        let obj = OwnedObjectId([1; 32]);
        let cert = cert_at(10, obj, [0xA; 32]);
        let ml = vec![MainLaneTx {
            round: 12,
            object: obj,
            payload_digest: [0xB; 32],
            lineage: CertHash([0; 32]),
        }];
        let proof = detect_fast_path_equivocation(&cert, &ml).unwrap();
        assert_eq!(proof.cert.tx.lineage_round, 10);
        assert_eq!(proof.conflicting_tx.round, 12);
    }

    #[test]
    fn conflict_outside_window_yields_no_proof() {
        let obj = OwnedObjectId([1; 32]);
        let cert = cert_at(10, obj, [0xA; 32]);
        // K = 4; window is (10, 14]. Round 15 is outside.
        let ml = vec![MainLaneTx {
            round: 15,
            object: obj,
            payload_digest: [0xB; 32],
            lineage: CertHash([0; 32]),
        }];
        assert!(detect_fast_path_equivocation(&cert, &ml).is_none());
    }
}
