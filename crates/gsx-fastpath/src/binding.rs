//! Main-lane binding window (K = 4 rounds, paper §6.4).
//!
//! "A fast-path certificate is binding subject to main-lane confirmation
//! within K rounds (target K = 4, ≈2 s)." We define binding precisely as:
//!
//! 1. A fast-path certified transaction `T` for object `O` with payload
//!    `P` and lineage at round `R`.
//! 2. The main lane within the window `(R, R + K]` either confirms `T`
//!    (a main-lane tx with the same `object`, `payload_digest`, and a
//!    lineage descended from `R`) or contains no transaction touching `O`.
//! 3. The certificate is *inconsistent* (signalling equivocation, DAG-S9)
//!    iff the main lane within the window contains a tx for `O` with a
//!    different `payload_digest` than `T`'s.
//!
//! Conflicts strictly beyond `R + K` are not the fast-path's
//! responsibility; the main-lane Mysticeti commit rule of DAG-S4
//! linearizes them.

use gsx_consensus::{CertHash, Round};
use serde::{Deserialize, Serialize};

use crate::cert::{FastPathCert, OwnedObjectId};

/// Main-lane confirmation depth `K` for fast-path binding (paper §6.4).
pub const FAST_PATH_CONFIRMATION_K: u32 = 4;

/// A transaction observed in the linearized main lane.
///
/// Phase-1 models the main lane as a flat sequence; in production this
/// is the output of `gsx_consensus::finalize`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MainLaneTx {
    /// Round at which this main-lane tx was committed.
    pub round: Round,
    /// Object touched.
    pub object: OwnedObjectId,
    /// Payload digest.
    pub payload_digest: [u8; 32],
    /// Lineage parent in the main-lane DAG.
    pub lineage: CertHash,
}

/// Check whether `cert` is consistent with the main lane.
///
/// Returns:
/// - `true` iff no conflicting main-lane tx exists in the binding window
///   `(cert.tx.lineage_round, cert.tx.lineage_round + K]`.
/// - `false` iff at least one main-lane tx in the window touches the
///   same object with a different payload digest (signal that the
///   fast-path signers equivocated).
pub fn is_main_lane_consistent(cert: &FastPathCert, main_lane: &[MainLaneTx]) -> bool {
    let lo = cert.tx.lineage_round + 1;
    let hi = cert.tx.lineage_round + FAST_PATH_CONFIRMATION_K as Round;
    for ml_tx in main_lane {
        if ml_tx.round < lo || ml_tx.round > hi {
            continue;
        }
        if ml_tx.object == cert.tx.object && ml_tx.payload_digest != cert.tx.payload_digest {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::cert::{FastPathTx, OwnedObjectId, OwnerAddress};

    fn cert_at(round: Round, object: OwnedObjectId, payload: [u8; 32]) -> FastPathCert {
        FastPathCert {
            tx: FastPathTx {
                object,
                owner: OwnerAddress([0; 32]),
                nonce: 0,
                lineage: CertHash::from([0; 32]),
                lineage_round: round,
                payload_digest: payload,
            },
            signers: BTreeSet::new(),
        }
    }

    #[test]
    fn k_matches_paper() {
        assert_eq!(FAST_PATH_CONFIRMATION_K, 4);
    }

    #[test]
    fn empty_main_lane_is_consistent() {
        let cert = cert_at(0, OwnedObjectId([1; 32]), [0xA; 32]);
        assert!(is_main_lane_consistent(&cert, &[]));
    }

    #[test]
    fn same_payload_in_window_is_consistent() {
        let obj = OwnedObjectId([1; 32]);
        let payload = [0xA; 32];
        let cert = cert_at(10, obj, payload);
        let ml = vec![MainLaneTx {
            round: 12,
            object: obj,
            payload_digest: payload,
            lineage: CertHash::from([0; 32]),
        }];
        assert!(is_main_lane_consistent(&cert, &ml));
    }

    #[test]
    fn conflicting_payload_in_window_is_inconsistent() {
        let obj = OwnedObjectId([1; 32]);
        let cert = cert_at(10, obj, [0xA; 32]);
        let ml = vec![MainLaneTx {
            round: 12,
            object: obj,
            payload_digest: [0xB; 32], // different payload
            lineage: CertHash::from([0; 32]),
        }];
        assert!(!is_main_lane_consistent(&cert, &ml));
    }

    #[test]
    fn conflict_beyond_window_is_consistent() {
        let obj = OwnedObjectId([1; 32]);
        let cert = cert_at(10, obj, [0xA; 32]);
        // K = 4 → window is (10, 14]. Round 15 is outside.
        let ml = vec![MainLaneTx {
            round: 15,
            object: obj,
            payload_digest: [0xB; 32],
            lineage: CertHash::from([0; 32]),
        }];
        assert!(is_main_lane_consistent(&cert, &ml));
    }

    #[test]
    fn conflict_on_different_object_is_consistent() {
        let cert = cert_at(10, OwnedObjectId([1; 32]), [0xA; 32]);
        let ml = vec![MainLaneTx {
            round: 12,
            object: OwnedObjectId([2; 32]), // different object
            payload_digest: [0xB; 32],
            lineage: CertHash::from([0; 32]),
        }];
        assert!(is_main_lane_consistent(&cert, &ml));
    }
}
