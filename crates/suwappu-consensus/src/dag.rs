//! In-memory certificate DAG store.
//!
//! Sprint scope (DAG-S3): the minimal viable DAG store that supports
//! insert + deterministic linearization. Certificates are never modified;
//! the only removal is bounded garbage collection (DAG-S34, IQ-008),
//! which evicts whole rounds at or below a monotone gc round.
//!
//! Validation on insert (paper §6.1):
//!
//! - Every parent hash must already be in the store, or be a tombstone of
//!   a certificate the store has pruned (no forward references).
//! - The certificate's round must be strictly greater than every parent's
//!   round (round monotonicity). Tombstones remember the pruned round so
//!   this check still runs for pruned parents.
//! - A round-0 certificate must have an empty parent set (genesis).
//! - The same certificate hash cannot be inserted twice.
//! - The certificate's round must be above the gc round.
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
//!
//! ## Garbage collection (IQ-008 D2)
//!
//! [`DagStore::prune_below`] evicts every certificate at or below the
//! given round and records a tombstone `(hash → round)` for each, kept
//! for a window of [`crate::gc::GC_DEPTH`] rounds below the gc round.
//! Round-driver parents are always the previous round, so after pruning
//! the only certificates that name pruned parents are those at
//! `gc_round + 1`; the tombstone window lets them validate instead of
//! being misclassified as orphans whose parents no peer can serve.
//! Memory is bounded by `authorities × GC_DEPTH` tombstones.

use std::collections::{BTreeMap, HashMap};

use crate::{
    cert::{CertHash, Certificate, Round},
    error::ConsensusError,
    gc::{is_obsolete, GC_DEPTH},
};

/// What one [`DagStore::prune_below`] call removed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PruneReport {
    /// Certificates evicted from the store.
    pub certs_pruned: usize,
    /// Rounds whose index entries were removed.
    pub rounds_pruned: usize,
    /// Tombstones that fell out of the retention window.
    pub tombstones_expired: usize,
    /// Hashes of every evicted certificate, so callers holding
    /// hash-keyed side tables (block payloads, votes, commit marks) can
    /// drop the matching entries in the same pass.
    pub evicted: Vec<CertHash>,
}

/// In-memory certificate DAG store.
#[derive(Debug, Default, Clone)]
pub struct DagStore {
    /// All certificates keyed by their content hash.
    certs: BTreeMap<CertHash, Certificate>,
    /// Inverted index: round → set of certificate hashes at that round.
    by_round: BTreeMap<Round, Vec<CertHash>>,
    /// Highest round pruned so far (`None` = nothing pruned).
    gc_round: Option<Round>,
    /// Pruned certificate → its round, for parent validation of
    /// certificates just above the gc round. Bounded by the tombstone
    /// window (see module docs).
    tombstones: HashMap<CertHash, Round>,
    /// Inverted tombstone index so expiry is O(rounds expired).
    tombstones_by_round: BTreeMap<Round, Vec<CertHash>>,
    /// Rounds of tombstones retained below the gc round. `0` means the
    /// default, [`GC_DEPTH`]; the daemon sets it to the manifest's
    /// `gc_depth_rounds` so the window tracks the mesh-wide depth.
    tombstone_window: Round,
}

impl DagStore {
    /// Construct an empty store with the default tombstone window
    /// ([`GC_DEPTH`] rounds).
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct an empty store whose tombstone window is `depth` rounds
    /// — the same depth the caller derives its gc round from, so a
    /// certificate at `gc_round + 1` can always validate against parents
    /// pruned in the most recent window. `0` falls back to [`GC_DEPTH`].
    pub fn with_gc_depth(depth: Round) -> Self {
        Self {
            tombstone_window: depth,
            ..Self::default()
        }
    }

    fn tombstone_window(&self) -> Round {
        if self.tombstone_window == 0 {
            GC_DEPTH
        } else {
            self.tombstone_window
        }
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

    /// The gc round: every round at or below it has been pruned.
    /// `None` until the first prune.
    pub fn gc_round(&self) -> Option<Round> {
        self.gc_round
    }

    /// Number of tombstones currently retained.
    pub fn tombstone_count(&self) -> usize {
        self.tombstones.len()
    }

    /// `true` iff `hash` was pruned and is still inside the tombstone
    /// window.
    pub fn is_tombstoned(&self, hash: &CertHash) -> bool {
        self.tombstones.contains_key(hash)
    }

    /// Iterate the retained tombstones as `(hash, round)`, rounds
    /// ascending. Used by the persistence layer (IQ-008 D4) to carry the
    /// window across a restart.
    pub fn tombstones(&self) -> impl Iterator<Item = (CertHash, Round)> + '_ {
        self.tombstones_by_round
            .iter()
            .flat_map(|(r, hs)| hs.iter().map(move |h| (*h, *r)))
    }

    /// Record a tombstone without ever having held the certificate:
    /// recovery replays committed certificates from the durable log, and
    /// a committed certificate whose parents fell below the floor before
    /// the snapshot cannot be `insert`ed, yet later certificates still
    /// name it as a parent. No-op for a hash that is live or already
    /// tombstoned, and for rounds above the gc round (those must be
    /// inserted properly). Returns whether a tombstone was added.
    pub fn insert_tombstone(&mut self, hash: CertHash, round: Round) -> bool {
        if self.certs.contains_key(&hash) || self.tombstones.contains_key(&hash) {
            return false;
        }
        if !is_obsolete(round, self.gc_round) {
            return false;
        }
        self.tombstones.insert(hash, round);
        self.tombstones_by_round
            .entry(round)
            .or_default()
            .push(hash);
        true
    }

    /// Set the gc round directly without evicting anything — for a
    /// store rebuilt from a snapshot, whose contents are already above
    /// the recorded gc round. Monotone like `prune_below`.
    pub fn restore_gc_round(&mut self, gc_round: Round) {
        if self.gc_round.map_or(true, |g| gc_round > g) {
            self.gc_round = Some(gc_round);
        }
    }

    /// Insert a certificate after validation. Returns the newly-inserted
    /// certificate's hash on success.
    pub fn insert(&mut self, cert: Certificate) -> Result<CertHash, ConsensusError> {
        // 0. Obsolete certificates are rejected outright (IQ-008 D1). They
        //    can no longer be part of any committed sub-DAG, and admitting
        //    one would re-create an unbounded tail below the gc round.
        if let Some(g) = self.gc_round {
            if is_obsolete(cert.round, Some(g)) {
                return Err(ConsensusError::BelowGcRound {
                    round: cert.round,
                    gc_round: g,
                });
            }
        }

        // 1. Genesis must carry no parents.
        if cert.round == 0 && !cert.parents.is_empty() {
            return Err(ConsensusError::GenesisWithParents);
        }

        // 2. Every parent must exist — live or tombstoned — with a strictly
        //    smaller round.
        for parent_hash in &cert.parents {
            let parent_round = match self.certs.get(parent_hash) {
                Some(parent) => parent.round,
                None => match self.tombstones.get(parent_hash) {
                    Some(r) => *r,
                    None => return Err(ConsensusError::UnknownParent(*parent_hash)),
                },
            };
            if parent_round >= cert.round {
                return Err(ConsensusError::NonMonotonicRound {
                    child: cert.round,
                    parent: parent_round,
                });
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

    /// Evict every certificate at or below `gc_round` and tombstone it
    /// (IQ-008 D2). Monotone: a `gc_round` at or below the current one is
    /// a no-op. Tombstones older than `GC_DEPTH` rounds below the new gc
    /// round are expired in the same call, so the store's footprint after
    /// pruning is bounded by the live window plus one tombstone window.
    ///
    /// Callers (the daemon's commit path) derive `gc_round` from the last
    /// committed leader via [`crate::gc::gc_round`]; the store itself
    /// applies no policy.
    pub fn prune_below(&mut self, gc_round: Round) -> PruneReport {
        if matches!(self.gc_round, Some(g) if gc_round <= g) {
            return PruneReport::default();
        }
        let mut report = PruneReport::default();

        // Split off every round <= gc_round. `split_off` keeps `>= key`, so
        // split at gc_round + 1 and swap.
        let keep = self.by_round.split_off(&gc_round.saturating_add(1));
        let evicted = std::mem::replace(&mut self.by_round, keep);
        for (round, hashes) in evicted {
            report.rounds_pruned += 1;
            for h in hashes {
                if self.certs.remove(&h).is_some() {
                    report.certs_pruned += 1;
                    report.evicted.push(h);
                }
                self.tombstones.insert(h, round);
                self.tombstones_by_round.entry(round).or_default().push(h);
            }
        }
        self.gc_round = Some(gc_round);

        // Expire tombstones below the window: keep rounds in
        // (gc_round - window, gc_round].
        if let Some(window_floor) = gc_round.checked_sub(self.tombstone_window()) {
            let keep = self
                .tombstones_by_round
                .split_off(&window_floor.saturating_add(1));
            let expired = std::mem::replace(&mut self.tombstones_by_round, keep);
            for (_, hashes) in expired {
                for h in hashes {
                    if self.tombstones.remove(&h).is_some() {
                        report.tombstones_expired += 1;
                    }
                }
            }
        }
        report
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
    /// `linearize().map(round).max()` because every `by_round` key has
    /// ≥1 cert. `insert` always pushes a hash, and `prune_below` removes
    /// whole rounds (never leaving an empty Vec behind) — those are the
    /// only two mutators. If a future change leaves an empty `by_round`
    /// Vec, `max_round`/`rounds`/`round_hashes` silently diverge from
    /// their linearize-derived equivalents and the commit-path
    /// substitution in `commit.rs` breaks. See `commit.rs::mod equivalence`
    /// and `proptest_gc.rs::store_size_is_bounded`, which asserts the
    /// no-empty-round property after pruning.
    pub fn max_round(&self) -> Option<Round> {
        self.by_round.keys().next_back().copied()
    }

    /// Iterate over all rounds present in the store, in ascending order.
    ///
    /// See [`Self::max_round`] for the non-empty-Vec invariant this
    /// relies on.
    pub fn rounds(&self) -> impl Iterator<Item = Round> + '_ {
        self.by_round.keys().copied()
    }

    /// Produce the deterministic linearization of the DAG.
    ///
    /// Order: rounds ascending; within a round, certificates sorted by
    /// `(authority_id, cert_hash)`.
    pub fn linearize(&self) -> Vec<CertHash> {
        let mut out = Vec::with_capacity(self.certs.len());
        for hashes in self.by_round.values() {
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

    fn child(author: u32, round: Round, parents: Vec<CertHash>, tag: u8) -> Certificate {
        Certificate {
            author,
            round,
            parents,
            payload_digest: [tag; 32],
            signature: Vec::new(),
        }
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

    // ── IQ-008 D2: pruning + tombstones ──────────────────────────────

    /// Three-round chain: g → r1 → r2, single author.
    fn chain() -> (DagStore, CertHash, CertHash, CertHash) {
        let mut s = DagStore::new();
        let g = s.insert(genesis(0)).unwrap();
        let r1 = s.insert(child(0, 1, vec![g], 1)).unwrap();
        let r2 = s.insert(child(0, 2, vec![r1], 2)).unwrap();
        (s, g, r1, r2)
    }

    #[test]
    fn prune_evicts_rounds_and_records_tombstones() {
        let (mut s, g, r1, r2) = chain();
        let rep = s.prune_below(1);
        assert_eq!(rep.certs_pruned, 2);
        assert_eq!(rep.rounds_pruned, 2);
        assert!(!s.contains(&g) && !s.contains(&r1) && s.contains(&r2));
        assert!(s.is_tombstoned(&g) && s.is_tombstoned(&r1));
        assert_eq!(s.gc_round(), Some(1));
        assert_eq!(s.max_round(), Some(2));
        assert_eq!(s.rounds().collect::<Vec<_>>(), vec![2]);
        assert_eq!(s.linearize(), vec![r2]);
    }

    #[test]
    fn prune_is_monotone_and_idempotent() {
        let (mut s, _, _, _) = chain();
        assert_eq!(s.prune_below(1).certs_pruned, 2);
        assert_eq!(s.prune_below(1), PruneReport::default());
        assert_eq!(s.prune_below(0), PruneReport::default());
        assert_eq!(s.gc_round(), Some(1));
    }

    #[test]
    fn insert_above_gc_round_accepts_tombstoned_parent() {
        let (mut s, _, r1, r2) = chain();
        s.prune_below(1);
        // A straggler at round 2 from another author whose parent (r1) was
        // pruned: still valid, because r1 is tombstoned with a known round.
        let straggler = child(1, 2, vec![r1], 0x22);
        assert!(s.insert(straggler).is_ok());
        // Monotonicity still enforced against the tombstone's round: a
        // cert at round 1 naming r1 (round 1) is below the gc round anyway,
        // so use round 2 naming r2 (round 2) — same-round parent.
        let bad = child(2, 2, vec![r2], 0x23);
        assert!(matches!(
            s.insert(bad),
            Err(ConsensusError::NonMonotonicRound {
                child: 2,
                parent: 2
            })
        ));
    }

    #[test]
    fn insert_at_or_below_gc_round_is_rejected() {
        let (mut s, g, _, _) = chain();
        s.prune_below(1);
        let late = child(3, 1, vec![g], 0x31);
        assert_eq!(
            s.insert(late),
            Err(ConsensusError::BelowGcRound {
                round: 1,
                gc_round: 1
            })
        );
        // Re-inserting a pruned genesis is likewise obsolete, not a
        // duplicate.
        assert!(matches!(
            s.insert(genesis(0)),
            Err(ConsensusError::BelowGcRound { round: 0, .. })
        ));
    }

    #[test]
    fn unknown_parent_still_detected_after_prune() {
        let (mut s, _, _, _) = chain();
        s.prune_below(1);
        let orphan = child(1, 2, vec![CertHash([0xEE; 32])], 0x24);
        assert!(matches!(
            s.insert(orphan),
            Err(ConsensusError::UnknownParent(_))
        ));
    }

    #[test]
    fn tombstones_expire_past_the_window() {
        // Build a single-author chain longer than GC_DEPTH, prune in two
        // steps, and check the tombstones for the oldest rounds are gone.
        let mut s = DagStore::new();
        let mut prev = s.insert(genesis(0)).unwrap();
        let mut hashes = vec![prev];
        let total = GC_DEPTH + 10;
        for r in 1..=total {
            prev = s.insert(child(0, r, vec![prev], (r % 251) as u8)).unwrap();
            hashes.push(prev);
        }
        // First prune: rounds <= 5. Tombstones for 0..=5 retained.
        s.prune_below(5);
        assert_eq!(s.tombstone_count(), 6);
        // Second prune: rounds <= GC_DEPTH + 5. Window keeps
        // (GC_DEPTH + 5 - GC_DEPTH, GC_DEPTH + 5] = (5, GC_DEPTH + 5], so
        // exactly GC_DEPTH tombstones remain and rounds 0..=5 are expired.
        let rep = s.prune_below(GC_DEPTH + 5);
        assert_eq!(rep.tombstones_expired, 6);
        assert_eq!(s.tombstone_count() as u64, GC_DEPTH);
        assert!(!s.is_tombstoned(&hashes[0]));
        assert!(!s.is_tombstoned(&hashes[5]));
        assert!(s.is_tombstoned(&hashes[6]));
        assert!(s.is_tombstoned(&hashes[GC_DEPTH as usize + 5]));
        assert_eq!(s.len() as u64, total - (GC_DEPTH + 5));
    }
}
