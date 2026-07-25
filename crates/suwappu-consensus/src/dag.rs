//! In-memory certificate DAG store.
//!
//! Sprint scope (DAG-S3): the minimal viable DAG store that supports
//! insert + deterministic linearization. The store is append-only;
//! certificates cannot be modified or removed.
//!
//! Validation on insert (paper §6.1):
//!
//! - Every parent hash must already be in the store (no forward references).
//! - The certificate's round must be strictly greater than every parent's
//!   round (round monotonicity).
//! - A round-0 certificate must have an empty parent set (genesis).
//! - The same certificate hash cannot be inserted twice.
//!
//! Linearization is deterministic:
//!
//! 1. Group certificates by round.
//! 2. Within each round, sort by `(authority_id, cert_hash)` for stable
//!    tie-breaking.
//! 3. Emit rounds in ascending order, certificates in the sorted order.
//!
//! Property `linearization_is_deterministic` (DAG-S3 exit gate): the
//! linearization of a DAG is invariant under insertion order. Verified at
//! 10,000 cases.

use std::collections::BTreeMap;

use crate::{
    cert::{CertHash, Certificate, Round},
    error::ConsensusError,
};

/// In-memory certificate DAG store.
#[derive(Debug, Default, Clone)]
pub struct DagStore {
    /// All certificates keyed by their content hash.
    certs: BTreeMap<CertHash, Certificate>,
    /// Inverted index: round → set of certificate hashes at that round.
    by_round: BTreeMap<Round, Vec<CertHash>>,
}

impl DagStore {
    /// Construct an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of certificates in the store.
    pub fn len(&self) -> usize {
        self.certs.len()
    }

    /// `true` iff the store is empty.
    pub fn is_empty(&self) -> bool {
        self.certs.is_empty()
    }

    /// `true` iff the store contains a certificate with the given hash.
    pub fn contains(&self, hash: &CertHash) -> bool {
        self.certs.contains_key(hash)
    }

    /// Borrow a certificate by hash.
    pub fn get(&self, hash: &CertHash) -> Option<&Certificate> {
        self.certs.get(hash)
    }

    /// Insert a certificate after validation. Returns the newly-inserted
    /// certificate's hash on success.
    pub fn insert(&mut self, cert: Certificate) -> Result<CertHash, ConsensusError> {
        // 1. Genesis must carry no parents.
        if cert.round == 0 && !cert.parents.is_empty() {
            return Err(ConsensusError::GenesisWithParents);
        }

        // 2. Every parent must exist with a strictly smaller round.
        for parent_hash in &cert.parents {
            match self.certs.get(parent_hash) {
                Some(parent) => {
                    if parent.round >= cert.round {
                        return Err(ConsensusError::NonMonotonicRound {
                            child: cert.round,
                            parent: parent.round,
                        });
                    }
                }
                None => return Err(ConsensusError::UnknownParent(*parent_hash)),
            }
        }

        // 3. Reject duplicate insertion.
        let hash = cert.hash();
        if self.certs.contains_key(&hash) {
            return Err(ConsensusError::DuplicateCertificate(hash));
        }

        // 4. Record.
        self.by_round.entry(cert.round).or_default().push(hash);
        self.certs.insert(hash, cert);
        Ok(hash)
    }

    /// Return all certificate hashes at the given round.
    ///
    /// The returned slice is in insertion order (not sorted); callers
    /// that need `(author, hash)` order must sort themselves.
    /// Returns an empty slice for rounds not present in the store.
    pub fn round_hashes(&self, round: Round) -> &[CertHash] {
        self.by_round.get(&round).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Return the highest round present in the store, or `None` if the
    /// store is empty.
    ///
    /// INVARIANT (load-bearing for the commit path): this equals
    /// `linearize().map(round).max()` only because `by_round` is append-only
    /// and every key has ≥1 cert — `insert` is the sole mutator and always
    /// pushes a hash. If a future change adds pruning/GC or otherwise leaves
    /// an empty `by_round` Vec, `max_round`/`rounds`/`round_hashes` silently
    /// diverge from their linearize-derived equivalents and the commit-path
    /// substitution in `commit.rs` breaks. See `commit.rs::mod equivalence`.
    pub fn max_round(&self) -> Option<Round> {
        self.by_round.keys().next_back().copied()
    }

    /// Iterate over all rounds present in the store, in ascending order.
    ///
    /// See [`Self::max_round`] for the append-only / non-empty-Vec invariant
    /// this relies on.
    pub fn rounds(&self) -> impl Iterator<Item = Round> + '_ {
        self.by_round.keys().copied()
    }

    /// Produce the deterministic linearization of the DAG.
    ///
    /// Order: rounds ascending; within a round, certificates sorted by
    /// `(authority_id, cert_hash)`.
    pub fn linearize(&self) -> Vec<CertHash> {
        let mut out = Vec::with_capacity(self.certs.len());
        for (_round, hashes) in self.by_round.iter() {
            let mut sorted = hashes.clone();
            sorted.sort_by_key(|h| {
                let cert = &self.certs[h];
                (cert.author, *h)
            });
            out.extend(sorted);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn genesis(author: u32) -> Certificate {
        Certificate::genesis(author, [author as u8; 32])
    }

    #[test]
    fn empty_store_has_zero_length() {
        let store = DagStore::new();
        assert!(store.is_empty());
        assert_eq!(store.linearize(), Vec::<CertHash>::new());
    }

    #[test]
    fn insert_genesis_succeeds() {
        let mut store = DagStore::new();
        let h = store.insert(genesis(0)).unwrap();
        assert!(store.contains(&h));
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn insert_genesis_with_parent_fails() {
        let mut store = DagStore::new();
        let mut g = genesis(0);
        g.parents.push(CertHash([0; 32]));
        assert_eq!(store.insert(g), Err(ConsensusError::GenesisWithParents),);
    }

    #[test]
    fn insert_with_unknown_parent_fails() {
        let mut store = DagStore::new();
        let cert = Certificate {
            author: 0,
            round: 1,
            parents: vec![CertHash([0xFF; 32])],
            payload_digest: [0; 32],
            signature: Vec::new(),
        };
        match store.insert(cert) {
            Err(ConsensusError::UnknownParent(_)) => {}
            other => panic!("expected UnknownParent, got {:?}", other),
        }
    }

    #[test]
    fn round_monotonicity_enforced() {
        let mut store = DagStore::new();
        let g_hash = store.insert(genesis(0)).unwrap();
        // A "child" certificate at round 0 referencing the genesis is
        // not monotonic — parent round 0 must be strictly less than the
        // child's round.
        let illegal = Certificate {
            author: 1,
            round: 0,
            parents: vec![g_hash],
            payload_digest: [1; 32],
            signature: Vec::new(),
        };
        // This first hits the genesis-with-parents rule (since round == 0),
        // which is the correct rejection. Test the monotonicity path with
        // a non-zero child round that references a same-round parent.
        let _ = store.insert(illegal);

        // Build a round-1 cert referencing genesis.
        let r1 = Certificate {
            author: 1,
            round: 1,
            parents: vec![g_hash],
            payload_digest: [1; 32],
            signature: Vec::new(),
        };
        let r1_hash = store.insert(r1).unwrap();

        // Another round-1 cert that references the round-1 parent — violates
        // monotonicity (parent round = child round).
        let bad = Certificate {
            author: 2,
            round: 1,
            parents: vec![r1_hash],
            payload_digest: [2; 32],
            signature: Vec::new(),
        };
        match store.insert(bad) {
            Err(ConsensusError::NonMonotonicRound {
                child: 1,
                parent: 1,
            }) => {}
            other => panic!("expected NonMonotonicRound, got {:?}", other),
        }
    }

    #[test]
    fn duplicate_insert_fails() {
        let mut store = DagStore::new();
        store.insert(genesis(5)).unwrap();
        assert!(matches!(
            store.insert(genesis(5)),
            Err(ConsensusError::DuplicateCertificate(_))
        ));
    }

    #[test]
    fn linearize_sorts_by_round_then_author() {
        let mut store = DagStore::new();
        // Insert three genesis certs in author order 2, 0, 1.
        let h2 = store.insert(genesis(2)).unwrap();
        let h0 = store.insert(genesis(0)).unwrap();
        let h1 = store.insert(genesis(1)).unwrap();

        let order = store.linearize();
        // Round 0; sorted by author → 0, 1, 2.
        assert_eq!(order, vec![h0, h1, h2]);
    }
}
