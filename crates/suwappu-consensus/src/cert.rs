//! Certificate types for the SUWAPPU certificate DAG.
//!
//! A `Certificate` is the unit of production in the Mysticeti-C DAG
//! (paper §6.1). Each Authority Node authors at most one certificate per
//! round, referencing certificates from prior rounds as parents.
//!
//! For DAG-S3 the certificate is a minimal data record: author, round,
//! parents, and a 32-byte payload digest (typically a block hash).
//!
//! DAG-S6 adds the signing surface: every certificate carries a detached
//! ML-DSA-65 signature (paper §6.1, reusing the Authority Ring signing
//! keys already used for bridge-header attestation, see
//! `bridge_header.rs`) over its own [`hash`](Certificate::hash). The
//! signature is authored with [`Certificate::sign`] and checked with
//! [`Certificate::verify_signature`] against the author's known public
//! key (resolved out-of-band, e.g. from the genesis manifest / Authority
//! Ring registry — `Certificate` itself carries no key material). Callers
//! that admit certificates into a [`crate::dag::DagStore`] from gossip or
//! RPC MUST call `verify_signature` and reject (never insert, never
//! re-gossip) on failure; `DagStore::insert` itself performs only the
//! structural DAG invariants (parent existence, round monotonicity,
//! genesis shape) and does not check signatures, so that its extensive
//! proptest/unit-test suite can keep constructing certificates directly
//! without needing key material.

use serde::{Deserialize, Serialize};
use suwappu_crypto::mldsa::{sign, verify, PublicKey, SecretKey};

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
    /// Detached ML-DSA-65 signature by `author` over `self.hash()`.
    ///
    /// Empty until [`Certificate::sign`] is called. `DagStore::insert`
    /// does not check this field — see the module docs for why signature
    /// verification is the admitting caller's responsibility.
    #[serde(default)]
    pub signature: Vec<u8>,
}

impl Certificate {
    /// Construct a genesis certificate (round 0, no parents, unsigned).
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

    /// Sign this certificate's [`hash`](Self::hash) with the author's
    /// ML-DSA-65 secret key and store the detached signature in
    /// `self.signature`.
    ///
    /// Signing over a freshly computed hash with a valid secret key is
    /// infallible, so this never fails.
    pub fn sign(&mut self, sk: &SecretKey) {
        let digest = self.hash();
        self.signature = sign(digest.as_bytes(), sk)
            .expect("ml-dsa-65 detached_sign over a valid secret key is infallible")
            .as_bytes()
            .to_vec();
    }

    /// Verify `self.signature` is a valid ML-DSA-65 detached signature by
    /// `pk` over `self.hash()`.
    ///
    /// Returns `false` (never panics) on empty/malformed signature bytes.
    /// This checks only the signature; it does NOT check that `pk`
    /// belongs to `self.author` in the Authority Ring — the caller must
    /// resolve `pk` from a trusted source (e.g. the genesis manifest)
    /// keyed by `self.author` before calling this.
    pub fn verify_signature(&self, pk: &PublicKey) -> bool {
        if self.signature.is_empty() {
            return false;
        }
        let digest = self.hash();
        match suwappu_crypto::mldsa::Signature::from_bytes(&self.signature) {
            Ok(sig) => verify(digest.as_bytes(), &sig, pk).is_ok(),
            Err(_) => false,
        }
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
            signature: Vec::new(),
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
            signature: Vec::new(),
        };
        let b = Certificate {
            author: 0,
            round: 1,
            parents: vec![p2, p1],
            payload_digest: [0; 32],
            signature: Vec::new(),
        };
        assert_ne!(a.hash(), b.hash());
    }

    #[test]
    fn sign_then_verify_succeeds() {
        let (pk, sk) = suwappu_crypto::mldsa::keypair();
        let mut c = Certificate::genesis(1, [0x11; 32]);
        assert!(!c.verify_signature(&pk), "unsigned certificate must not verify");
        c.sign(&sk);
        assert!(c.verify_signature(&pk), "honestly-signed certificate must verify");
    }

    #[test]
    fn verify_rejects_wrong_key() {
        let (_pk_a, sk_a) = suwappu_crypto::mldsa::keypair();
        let (pk_b, _sk_b) = suwappu_crypto::mldsa::keypair();
        let mut c = Certificate::genesis(1, [0x22; 32]);
        c.sign(&sk_a);
        assert!(!c.verify_signature(&pk_b), "signature by a different key must not verify");
    }

    #[test]
    fn verify_rejects_tampered_certificate() {
        let (pk, sk) = suwappu_crypto::mldsa::keypair();
        let mut c = Certificate::genesis(1, [0x33; 32]);
        c.sign(&sk);
        c.payload_digest[0] ^= 0x01;
        assert!(!c.verify_signature(&pk), "tampering after signing must break verification");
    }
}
