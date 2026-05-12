//! Fast-path certificate types.

use std::collections::BTreeSet;

use gsx_consensus::{AuthorityId, CertHash, Round};
use serde::{Deserialize, Serialize};

/// Move object identifier — the unit on which fast-path eligibility is
/// gated. Paper §6.4: "read-write footprint is a single owned Move object."
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct OwnedObjectId(pub [u8; 32]);

/// Owner address — the sole signer of a fast-path transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct OwnerAddress(pub [u8; 32]);

/// A fast-path transaction.
///
/// Eligibility (paper §6.4):
/// - Single `object` read-write footprint.
/// - `owner` is sole signer.
/// - `lineage` references a certificate already linearized in the main-lane DAG.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FastPathTx {
    /// Owned Move object touched by this transaction.
    pub object: OwnedObjectId,
    /// Sole signer.
    pub owner: OwnerAddress,
    /// Per-object monotonic nonce. Replay protection.
    pub nonce: u64,
    /// Main-lane certificate hash this transaction descends from.
    pub lineage: CertHash,
    /// Round of the main-lane lineage certificate.
    pub lineage_round: Round,
    /// 32-byte content digest of the transaction payload.
    pub payload_digest: [u8; 32],
}

/// A fast-path certificate produced by an Authority Ring quorum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FastPathCert {
    /// The underlying transaction.
    pub tx: FastPathTx,
    /// Authority Ring members that signed. Quorum check requires
    /// `|signers| >= fast_path_quorum_size(|A|)`.
    pub signers: BTreeSet<AuthorityId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_path_tx_constructs() {
        let tx = FastPathTx {
            object: OwnedObjectId([0xAB; 32]),
            owner: OwnerAddress([0xCD; 32]),
            nonce: 42,
            lineage: CertHash([0xEF; 32]),
            lineage_round: 7,
            payload_digest: [0x11; 32],
        };
        assert_eq!(tx.nonce, 42);
        assert_eq!(tx.lineage_round, 7);
    }
}
