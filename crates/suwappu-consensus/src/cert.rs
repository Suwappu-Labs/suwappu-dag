//! Certificate types for the SUWAPPU certificate DAG.
//!
//! A `Certificate` is the unit of production in the DagBft-C DAG
//! (paper §6.1). Each Authority Node authors at most one certificate per
//! round, referencing certificates from prior rounds as parents.
//!
//! For DAG-S3 the certificate is a minimal data record: author, round,
//! parents, and a 32-byte payload digest (typically a block hash). The
//! signing surface lands in DAG-S6 with the validator-set registry.

use serde::{Deserialize, Serialize};

/// Authority identifier — index into the published Authority Ring set.
///
/// Phase-1 uses a `u32` index assigned at admission. The full
/// PoA-Node public-key binding lives in `suwappu-authority`.
pub type AuthorityId = u32;

/// Consensus round number.
///
/// Round 0 is reserved for genesis certificates (one per authority). Round
/// monotonicity is a hard invariant: a certificate at round `R` may only
/// reference parents at rounds strictly less than `R`.
pub type Round = u64;

/// Content-addressed hash of a certificate's canonical encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CertHash(pub [u8; 32]);

impl CertHash {
    /// Borrow the underlying 32 bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// A single DAG certificate.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Certificate {
    /// Authoring Authority Ring member.
    pub author: AuthorityId,
    /// Consensus round.
    pub round: Round,
    /// Parent certificate hashes. Empty iff `round == 0`.
    pub parents: Vec<CertHash>,
    /// 32-byte digest of the payload (typically a block hash).
    pub payload_digest: [u8; 32],
}

impl Certificate {
    /// Construct a genesis certificate (round 0, no parents).
    pub fn genesis(author: AuthorityId, payload_digest: [u8; 32]) -> Self {
        Self {
            author,
            round: 0,
            parents: Vec::new(),
            payload_digest,
        }
    }

    /// Compute the canonical hash of this certificate using BLAKE3 over a
    /// domain-separated, deterministic encoding.
    ///
    /// Encoding: `tag || author (4 BE) || round (8 BE) || parent_count
    /// (4 BE) || parents[0..n] || payload_digest`.
    pub fn hash(&self) -> CertHash {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"SUWAPPU-CERT-V1");
        hasher.update(&self.author.to_be_bytes());
        hasher.update(&self.round.to_be_bytes());
        hasher.update(&(self.parents.len() as u32).to_be_bytes());
        for parent in &self.parents {
            hasher.update(&parent.0);
        }
        hasher.update(&self.payload_digest);
        let mut out = [0u8; 32];
        out.copy_from_slice(hasher.finalize().as_bytes());
        CertHash(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genesis_has_no_parents() {
        let g = Certificate::genesis(7, [0xAB; 32]);
        assert_eq!(g.round, 0);
        assert!(g.parents.is_empty());
        assert_eq!(g.author, 7);
    }

    #[test]
    fn hash_is_deterministic() {
        let c = Certificate {
            author: 3,
            round: 5,
            parents: vec![CertHash([1; 32]), CertHash([2; 32])],
            payload_digest: [0xCD; 32],
        };
        assert_eq!(c.hash(), c.hash());
    }

    #[test]
    fn hash_distinguishes_authors() {
        let a = Certificate::genesis(1, [0; 32]);
        let b = Certificate::genesis(2, [0; 32]);
        assert_ne!(a.hash(), b.hash());
    }

    #[test]
    fn hash_distinguishes_parent_order() {
        let p1 = CertHash([1; 32]);
        let p2 = CertHash([2; 32]);
        let a = Certificate {
            author: 0,
            round: 1,
            parents: vec![p1, p2],
            payload_digest: [0; 32],
        };
        let b = Certificate {
            author: 0,
            round: 1,
            parents: vec![p2, p1],
            payload_digest: [0; 32],
        };
        assert_ne!(a.hash(), b.hash());
    }
}
