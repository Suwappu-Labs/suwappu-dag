//! Certificate types for the GSX certificate DAG.
//!
//! A `Certificate` is the unit of production in the Mysticeti-C DAG
//! (paper §6.1). Each Authority Node authors at most one certificate per
//! round, referencing certificates from prior rounds as parents.
//!
//! Each certificate carries an ML-DSA-65 detached signature over its
//! content hash (`cert.hash(network_id)`). The signature binds the `author` field
//! to a cryptographic identity in the Authority Registry — without it,
//! any peer could forge a certificate claiming any author.
//!
//! Signature verification is performed by the daemon before DAG
//! insertion; `DagStore` itself validates only structural invariants
//! (parent refs, round monotonicity, dedup).

use serde::{Deserialize, Serialize};

/// Authority identifier — index into the published Authority Ring set.
///
/// Phase-1 uses a `u32` index assigned at admission. The full
/// PoA-Node public-key binding lives in `gsx-authority`.
pub type AuthorityId = u32;

/// Consensus round number.
///
/// Round 0 is reserved for genesis certificates (one per authority). Round
/// monotonicity is a hard invariant: a certificate at round `R` may only
/// reference parents at rounds strictly less than `R`.
pub type Round = u64;

/// Content-addressed hash of a certificate's canonical encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CertHash([u8; 32]);

impl CertHash {
    /// Borrow the underlying 32 bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl AsRef<[u8; 32]> for CertHash {
    fn as_ref(&self) -> &[u8; 32] {
        &self.0
    }
}

impl From<[u8; 32]> for CertHash {
    fn from(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl From<CertHash> for [u8; 32] {
    fn from(h: CertHash) -> Self {
        h.0
    }
}

impl std::fmt::Display for CertHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "0x")?;
        for byte in &self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
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
    /// ML-DSA-65 detached signature over `self.hash(network_id)`.
    ///
    /// The signature proves that the holder of the Authority Ring
    /// member's secret key authorized this certificate. Verification
    /// is performed by the daemon via [`verify_cert_signature`] before
    /// DAG insertion — `DagStore::insert` does not check signatures.
    ///
    /// Empty in unit tests that exercise DAG topology only.
    pub signature: Vec<u8>,
}

impl Certificate {
    /// Construct an unsigned genesis certificate (round 0, no parents).
    ///
    /// The caller must set `signature` before broadcasting. For
    /// topology-only tests where signing is irrelevant, the empty
    /// signature is sufficient (DagStore does not verify signatures).
    pub fn genesis(author: AuthorityId, payload_digest: [u8; 32]) -> Self {
        Self {
            author,
            round: 0,
            parents: Vec::new(),
            payload_digest,
            signature: Vec::new(),
        }
    }

    /// Compute the canonical hash of this certificate using BLAKE3 over a
    /// domain-separated, deterministic encoding.
    ///
    /// Encoding: `tag || network_id || author (4 BE) || round (8 BE) ||
    /// parent_count (4 BE) || parents[0..n] || payload_digest`.
    ///
    /// `network_id` prevents cross-network replay: the same certificate
    /// content on devnet and testnet produces different hashes, so a
    /// signature valid on one network cannot be replayed on another.
    pub fn hash(&self, network_id: &str) -> CertHash {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"GSX-CERT-V1");
        hasher.update(network_id.as_bytes());
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

    const NET: &str = "test";

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
            parents: vec![CertHash::from([1; 32]), CertHash::from([2; 32])],
            payload_digest: [0xCD; 32],
            signature: vec![],
        };
        assert_eq!(c.hash(NET), c.hash(NET));
    }

    #[test]
    fn hash_distinguishes_authors() {
        let a = Certificate::genesis(1, [0; 32]);
        let b = Certificate::genesis(2, [0; 32]);
        assert_ne!(a.hash(NET), b.hash(NET));
    }

    #[test]
    fn hash_excludes_signature() {
        let a = Certificate {
            author: 3,
            round: 5,
            parents: vec![CertHash::from([1; 32])],
            payload_digest: [0xCD; 32],
            signature: vec![0xAA; 64],
        };
        let b = Certificate {
            author: 3,
            round: 5,
            parents: vec![CertHash::from([1; 32])],
            payload_digest: [0xCD; 32],
            signature: vec![0xBB; 128],
        };
        assert_eq!(a.hash(NET), b.hash(NET));
    }

    #[test]
    fn hash_distinguishes_parent_order() {
        let p1 = CertHash::from([1; 32]);
        let p2 = CertHash::from([2; 32]);
        let a = Certificate {
            author: 0,
            round: 1,
            parents: vec![p1, p2],
            payload_digest: [0; 32],
            signature: vec![],
        };
        let b = Certificate {
            author: 0,
            round: 1,
            parents: vec![p2, p1],
            payload_digest: [0; 32],
            signature: vec![],
        };
        assert_ne!(a.hash(NET), b.hash(NET));
    }

    #[test]
    fn hash_distinguishes_network_ids() {
        let c = Certificate::genesis(0, [0; 32]);
        assert_ne!(
            c.hash("devnet"),
            c.hash("testnet"),
            "same cert must hash differently on different networks"
        );
    }
}
