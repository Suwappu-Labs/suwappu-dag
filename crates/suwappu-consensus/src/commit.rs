//! Mysticeti-C commit rule over the certificate DAG.
//!
//! Implements §6.2 of the *SUWAPPU DAG Layer 1* paper: deterministic finality
//! through an uncertified DAG with a novel commit rule
//! [Babel et al., 2023].
//!
//! Sprint scope (DAG-S4): the Authority-side commit rule. Validator-Ring
//! ratification (Theorem 2's joint-quorum AND-gate) lands in DAG-S5.
//!
//! ## Commit rule
//!
//! Let `n` be the Authority-Ring size and `q = ⌈2n/3⌉ + 1` the
//! Byzantine-fault-tolerant supermajority threshold (Definition 2).
//!
//! 1. **Leader selection.** At each round `r ≥ 0`, the leader is the
//!    Authority Node whose `AuthorityId` satisfies `id ≡ r (mod n)`.
//!    This is deterministic and round-robin.
//! 2. **Support.** A certificate at round `r+1` *supports* the leader's
//!    certificate at round `r` iff it lists the leader's certificate
//!    hash among its parents.
//! 3. **Commit.** The leader at round `r` is committed iff at least `q`
//!    distinct supporters (by author) exist at round `r+1`.
//! 4. **Causal history.** Once a leader is committed, its entire causal
//!    history (every ancestor reachable by walking parent pointers) is
//!    finalized in the deterministic order of `DagStore::linearize()`.
//!
//! ## Finality (exit gate)
//!
//! Once a leader is committed, no DAG extension (insertion of additional
//! valid certificates) can uncommit it: the supporter set at `r+1` only
//! grows, never shrinks. Verified at 10,000 cases by
//! `mysticeti_c_finality`.

use std::collections::{BTreeSet, HashSet, VecDeque};

use crate::{
    cert::{AuthorityId, CertHash, Round},
    dag::DagStore,
};

/// Authority-Ring size — number of registered authorities.
///
/// Phase-1 takes this as a parameter to the commit rule; the
/// validator-set registry that publishes it lands in DAG-S6.
pub type CommitteeSize = u32;

/// Byzantine-fault-tolerant supermajority threshold for an Authority Ring
/// of size `n`: `q = 2f + 1` where `f = ⌊(n-1)/3⌋` — equivalently
/// `q = n − ⌊(n-1)/3⌋`. This is the canonical Mysticeti / Bullshark /
/// DAG-Rider threshold and is what Sui Lutris ships
/// (`consensus/config/src/committee.rs`).
///
/// **Divergence from paper §6.4.** The paper writes `q = ⌈2n/3⌉ + 1`, but
/// for `n` where `2n mod 3 = 2` (n ∈ {1, 4, 7, …}) that formula collapses
/// to unanimity, which has no Byzantine fault tolerance. The May-13 perf
/// testnet (n=4) committed exactly once in 9 hours because of this. See
/// `docs/iq/IQ-001-quorum-formula.md` for the resolution.
///
/// Definition 2's "strict majority of 2/3" inequality is satisfied by
/// `2f+1` for any `n = 3f + 1`, so Theorem 2's safety proof is unchanged.
pub fn quorum_threshold(n: CommitteeSize) -> u32 {
    if n == 0 {
        return 1;
    }
    n - (n - 1) / 3
}

/// Deterministic round-robin leader for `round` given `n` authorities.
pub fn leader(round: Round, n: CommitteeSize) -> AuthorityId {
    assert!(n > 0, "committee size must be > 0");
    (round % n as u64) as AuthorityId
}

/// Return the certificate hash authored by `author` at `round` in `dag`,
/// if it exists. There is at most one (round, author) certificate in an
/// honest DAG; equivocation detection lands in DAG-S7.
///
/// Under equivocation (multiple certs with the same (round, author)) the
/// returned hash is the minimum `CertHash` among them — identical to the
/// original `linearize().find()` which emits within-round certs in
/// `(author, hash)` ascending order and therefore returns the min hash first.
pub fn cert_at(dag: &DagStore, round: Round, author: AuthorityId) -> Option<CertHash> {
    dag.round_hashes(round)
        .iter()
        .copied()
        .filter(|h| {
            dag.get(h)
                .expect("hash from round_hashes must resolve")
                .author
                == author
        })
        .min()
}

/// Return the set of distinct authors at `round` who directly reference
/// `target` as one of their parents.
fn supporters(dag: &DagStore, target: CertHash, round: Round) -> BTreeSet<AuthorityId> {
    dag.round_hashes(round)
        .iter()
        .filter_map(|h| {
            let c = dag.get(h).expect("hash from round_hashes must resolve");
            if c.parents.contains(&target) {
                Some(c.author)
            } else {
                None
            }
        })
        .collect()
}

/// Decision for a single leader slot under the Mysticeti-C commit rule.
///
/// - `Direct(hash)` — leader directly committed by the direct rule, or
///   inherited from a later anchor's causal history by the indirect rule.
/// - `Skip` — leader is permanently skipped (no cert ever finalizes).
/// - `Undecided` — neither rule has fired yet for this slot; may resolve
///   later when a higher-round anchor commits.
///
/// See `docs/iq/IQ-002-indirect-commit.md` for the protocol-level rationale.
///
/// Deliberately **not** `#[non_exhaustive]`: the three states are the
/// canonical paper-defined decision outcomes (paper §6 + Theorem 2).
/// Adding a fourth state would require a paper-level amendment and
/// therefore a major-version bump, so downstream exhaustive matching
/// is the desired behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaderStatus {
    /// Leader committed (directly or by inheritance from a later anchor).
    Direct(CertHash),
    /// Leader permanently skipped — no cert at this slot was ever in any
    /// directly-decided anchor's causal history.
    Skip,
    /// Decision pending. The slot may resolve to `Direct` or `Skip` once
    /// a higher-round anchor at round ≥ `round + 2` is directly decided.
    Undecided,
}

/// Direct commit rule. If the leader's cert at `round` has at least
/// `quorum_threshold(n)` distinct supporters at `round + 1`, the leader
/// is directly committed. Otherwise undecided.
///
/// This is the round-R+1 fast path. The 3-round-wave's explicit
/// `Skip` (when ≥ quorum decision-round certs blame the leader) is
/// folded into the indirect rule for simplicity — see
/// [`try_indirect_decide`] and [`decide_slot`].
pub fn try_direct_decide(dag: &DagStore, round: Round, n: CommitteeSize) -> LeaderStatus {
    let author = leader(round, n);
    let leader_hash = match cert_at(dag, round, author) {
        Some(h) => h,
        None => return LeaderStatus::Undecided,
    };
    let support = supporters(dag, leader_hash, round + 1);
    if support.len() as u32 >= quorum_threshold(n) {
        LeaderStatus::Direct(leader_hash)
    } else {
        LeaderStatus::Undecided
    }
}

/// Indirect commit rule. Given a directly-decided anchor (its cert
/// hash), determine the status of an earlier leader slot at
/// `target_round` by walking the anchor's causal history. If the
/// leader's cert at `target_round` is reachable from `anchor`, it
/// inherits a `Direct` decision; otherwise it is permanently `Skip`.
///
/// Pre-condition: `anchor` MUST have been directly decided (i.e., the
/// caller has confirmed `try_direct_decide(dag, anchor_round, n) ==
/// Direct(anchor)` at some round > target_round).
pub fn try_indirect_decide(
    dag: &DagStore,
    target_round: Round,
    anchor: CertHash,
    n: CommitteeSize,
) -> LeaderStatus {
    let target_author = leader(target_round, n);
    let target_hash = match cert_at(dag, target_round, target_author) {
        Some(h) => h,
        // No cert was ever authored at this slot — definitively skipped.
        None => return LeaderStatus::Skip,
    };
    if causal_history(dag, anchor).contains(&target_hash) {
        LeaderStatus::Direct(target_hash)
    } else {
        LeaderStatus::Skip
    }
}

/// Top-level slot decision. Tries direct commit first; on `Undecided`,
/// scans directly-decided anchors at round `target_round + 2 ..= max_round`
/// and applies the indirect rule against each.
///
/// **IQ-004 late-arrival fix (closes #45).** Previously, this function
/// returned on the first directly-decided anchor — so if that anchor's
/// causal history didn't yet include the target leader cert, the slot
/// was reported `Skip` permanently. Under jitter + orphan-pull recovery,
/// a leader cert can arrive at peers *after* the round-R+1 wave has
/// already proposed without it as a parent, but later certs (e.g., at
/// round R+2 or beyond) may reference it via certs that DO include it.
/// A later directly-decided anchor's `causal_history` can then reach
/// the leader cert, where the first anchor's could not.
///
/// We therefore iterate every directly-decided anchor in ascending round
/// order. The first anchor whose `try_indirect_decide` yields
/// `Direct(leader_hash)` wins and we return that. If every directly-
/// decided anchor yields `Skip`, we return `Skip`. If no directly-
/// decided anchor exists yet (or none has been found within the search
/// range), we return `Undecided` — i.e., the slot can still be resolved
/// once a future anchor is directly decided.
///
/// Safety: `Direct → Skip/Undecided` transitions remain forbidden
/// (verified by `mysticeti_c_finality` at 10k cases). Only
/// `Skip → Direct` is newly permitted via this late-arrival path. The
/// safety witness for any retroactive flip is the directly-decided
/// anchor itself, which is finalized via the joint-quorum AND-gate
/// before its causal history can reach the late leader cert.
pub fn decide_slot(dag: &DagStore, target_round: Round, n: CommitteeSize) -> LeaderStatus {
    match try_direct_decide(dag, target_round, n) {
        LeaderStatus::Direct(h) => return LeaderStatus::Direct(h),
        LeaderStatus::Skip => return LeaderStatus::Skip,
        LeaderStatus::Undecided => {}
    }
    let max_round = match dag.max_round() {
        Some(r) => r,
        None => return LeaderStatus::Undecided,
    };
    // Search every anchor at R' >= target+2 in ascending order. The
    // "+2" gap is the canonical wave length: direct commit at R'
    // relies on supporters at R'+1, so an anchor at R' = target+2
    // means target's leader cert is in R'+1's parent set if any
    // honest authority at R+1 supported it. Smaller anchors
    // (R'=target+1) are the leader itself and not informative.
    //
    // We do NOT early-return on the first anchor's Skip — see IQ-004.
    let mut any_anchor_seen = false;
    for anchor_round in (target_round + 2)..=max_round {
        if let LeaderStatus::Direct(anchor_hash) = try_direct_decide(dag, anchor_round, n) {
            any_anchor_seen = true;
            match try_indirect_decide(dag, target_round, anchor_hash, n) {
                LeaderStatus::Direct(h) => return LeaderStatus::Direct(h),
                // This anchor's causal history doesn't reach the leader
                // cert. Keep scanning — a later anchor may, via certs
                // that have arrived since.
                LeaderStatus::Skip | LeaderStatus::Undecided => continue,
            }
        }
    }
    // No anchor's causal history reaches the leader. If we found at
    // least one directly-decided anchor, treat the slot as `Skip`
    // (sufficient evidence: the cluster moved past R without
    // anyone's causal chain referencing the leader cert). If we
    // haven't seen any directly-decided anchor yet, the slot is
    // still `Undecided` — a future anchor may resolve it.
    if any_anchor_seen {
        LeaderStatus::Skip
    } else {
        LeaderStatus::Undecided
    }
}

/// Back-compat wrapper. `Some(hash)` iff [`decide_slot`] returns
/// `Direct(hash)`. New code should call `decide_slot` directly so it
/// can distinguish `Skip` from `Undecided`.
pub fn commit_leader(dag: &DagStore, round: Round, n: CommitteeSize) -> Option<CertHash> {
    match decide_slot(dag, round, n) {
        LeaderStatus::Direct(h) => Some(h),
        _ => None,
    }
}

/// Walk the DAG from `start` and collect every ancestor (including
/// `start` itself). Returns the ancestor set as a deterministic
/// linearized vector following `DagStore::linearize()`'s order:
/// round-ascending, within a round `(author, hash)` ascending.
///
/// The BFS collects `seen` hashes; we then filter to in-DAG entries
/// (same as `linearize().filter(seen.contains)` — linearize only emits
/// certs actually in the store) and sort by `(round, author, hash)`,
/// which reproduces the linearize order exactly.
pub fn causal_history(dag: &DagStore, start: CertHash) -> Vec<CertHash> {
    let mut seen: HashSet<CertHash> = HashSet::new();
    let mut queue: VecDeque<CertHash> = VecDeque::new();
    queue.push_back(start);
    while let Some(h) = queue.pop_front() {
        if !seen.insert(h) {
            continue;
        }
        if let Some(c) = dag.get(&h) {
            for p in &c.parents {
                queue.push_back(*p);
            }
        }
    }
    // Filter to certs that are actually in the DAG (mirrors linearize's
    // implicit filter) then sort by (round, author, hash) to reproduce
    // linearize order.
    let mut out: Vec<CertHash> = seen.into_iter().filter(|h| dag.contains(h)).collect();
    // FORK-CRITICAL: the trailing `*h` is the only thing that makes this
    // order TOTAL. `seen` is a HashSet (nondeterministic iteration), and two
    // equivocating certs can share the same (round, author). Without `*h`,
    // sort_by_key would leave their relative order unspecified → different
    // nodes could linearize the same anchor's causal history differently →
    // consensus fork. Do not drop the `*h` tie-breaker.
    out.sort_by_key(|h| {
        let c = dag.get(h).expect("just filtered to in-dag hashes");
        (c.round, c.author, *h)
    });
    out
}

/// Run the Mysticeti-C commit rule (direct + indirect) across every
/// round of the DAG and return the finalized linear history.
///
/// The output is the deterministic concatenation of causal histories of
/// every `LeaderStatus::Direct` slot — whether decided directly or
/// inherited via the indirect rule — deduplicated to preserve
/// append-only finality.
pub fn finalize(dag: &DagStore, n: CommitteeSize) -> Vec<CertHash> {
    let max_round = match dag.max_round() {
        Some(r) => r,
        None => return Vec::new(),
    };

    let mut finalized: Vec<CertHash> = Vec::new();
    let mut included: HashSet<CertHash> = HashSet::new();

    // Iterate rounds in ascending order. `decide_slot` consults the
    // indirect rule for each, so an undecided round at evaluation time
    // can still resolve here if a later anchor (within the same DAG
    // snapshot) is directly decided.
    for r in 0..max_round {
        if let LeaderStatus::Direct(leader_hash) = decide_slot(dag, r, n) {
            for h in causal_history(dag, leader_hash) {
                if included.insert(h) {
                    finalized.push(h);
                }
            }
        }
    }
    finalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cert::Certificate;

    fn genesis(author: u32) -> Certificate {
        Certificate::genesis(author, [author as u8; 32])
    }

    fn child(author: u32, round: Round, parents: Vec<CertHash>, tag: u8) -> Certificate {
        Certificate {
            author,
            round,
            parents,
            payload_digest: [tag; 32],
        }
    }

    #[test]
    fn quorum_matches_canonical_bft() {
        // q = 2f+1, f = ⌊(n-1)/3⌋. Matches Sui / Mysticeti / Bullshark.
        // See IQ-001 for divergence from paper §6.4's `⌈2n/3⌉+1`.
        assert_eq!(quorum_threshold(1), 1); // f=0, 2f+1=1
        assert_eq!(quorum_threshold(3), 3); // f=0
        assert_eq!(quorum_threshold(4), 3); // f=1, 2f+1=3 (was 4 under paper)
        assert_eq!(quorum_threshold(6), 5); // f=1
        assert_eq!(quorum_threshold(7), 5); // f=2 (was 6 under paper)
        assert_eq!(quorum_threshold(10), 7); // f=3 (was 8 under paper)
        assert_eq!(quorum_threshold(13), 9); // f=4 (was 10 under paper)

        // Property: q > 2n/3 (strict supermajority — Definition 2) for all n.
        for n in 1u32..=64 {
            let q = quorum_threshold(n);
            assert!(3 * q > 2 * n, "n={n}: q={q} fails strict 2/3 majority");
            assert!(q <= n, "n={n}: q={q} exceeds committee size");
        }
    }

    #[test]
    fn leader_is_round_robin() {
        assert_eq!(leader(0, 4), 0);
        assert_eq!(leader(1, 4), 1);
        assert_eq!(leader(4, 4), 0);
        assert_eq!(leader(7, 4), 3);
    }

    #[test]
    fn single_authority_commits_trivially() {
        // n = 1: quorum_threshold is 1. The leader at round 0 is author 0;
        // a single supporter at round 1 commits it.
        let mut dag = DagStore::new();
        let g = dag.insert(genesis(0)).unwrap();
        let _r1 = dag.insert(child(0, 1, vec![g], 0xAA)).unwrap();
        assert_eq!(commit_leader(&dag, 0, 1), Some(g));
    }

    #[test]
    fn missing_leader_is_not_committed() {
        // n = 4, leader at round 0 is author 0. If only author 1 produces
        // a genesis cert, no commit is possible.
        let mut dag = DagStore::new();
        let _ = dag.insert(genesis(1)).unwrap();
        assert_eq!(commit_leader(&dag, 0, 4), None);
    }

    #[test]
    fn insufficient_supporters_is_not_committed() {
        // n = 4, q = 3 (2f+1 with f=1). Two supporters → below quorum.
        let mut dag = DagStore::new();
        let mut g_hashes = Vec::new();
        for a in 0..4 {
            g_hashes.push(dag.insert(genesis(a)).unwrap());
        }
        let leader_hash = g_hashes[0];
        dag.insert(child(1, 1, vec![leader_hash], 0x11)).unwrap();
        dag.insert(child(2, 1, vec![leader_hash], 0x22)).unwrap();
        assert_eq!(commit_leader(&dag, 0, 4), None);
    }

    #[test]
    fn sufficient_supporters_commits() {
        // n = 4, q = 3. Three supporters → commit. Tolerates one offline.
        let mut dag = DagStore::new();
        let mut g_hashes = Vec::new();
        for a in 0..4 {
            g_hashes.push(dag.insert(genesis(a)).unwrap());
        }
        let leader_hash = g_hashes[0];
        for a in 0..3 {
            dag.insert(child(a, 1, vec![leader_hash], a as u8)).unwrap();
        }
        assert_eq!(commit_leader(&dag, 0, 4), Some(leader_hash));
    }

    #[test]
    fn causal_history_includes_self_and_ancestors() {
        let mut dag = DagStore::new();
        let g0 = dag.insert(genesis(0)).unwrap();
        let g1 = dag.insert(genesis(1)).unwrap();
        let r1 = dag.insert(child(0, 1, vec![g0, g1], 0xCC)).unwrap();

        let history = causal_history(&dag, r1);
        assert!(history.contains(&r1));
        assert!(history.contains(&g0));
        assert!(history.contains(&g1));
        assert_eq!(history.len(), 3);
    }

    #[test]
    fn finalize_empty_dag_is_empty() {
        let dag = DagStore::new();
        assert!(finalize(&dag, 4).is_empty());
    }

    #[test]
    fn undecided_slot_resolves_via_later_anchor() {
        // n=4, q=3. Build a DAG where round 0's leader has only 2 direct
        // supporters (Undecided by direct rule), but round 2's leader is
        // directly committed AND its causal history includes round 0's
        // leader cert. Indirect rule should resolve round 0 to Direct.
        let mut dag = DagStore::new();
        let mut g = Vec::new();
        for a in 0..4 {
            g.push(dag.insert(genesis(a)).unwrap());
        }
        let r0_leader = g[0]; // leader(0, 4) = 0

        // Round 1: only 2 supporters of r0_leader (authors 0 and 1).
        // Other parents of those round-1 certs include the other genesis
        // certs so we can author round 2 against them.
        let r1_0 = dag
            .insert(child(0, 1, vec![g[0], g[1], g[2], g[3]], 0x10))
            .unwrap();
        let r1_1 = dag
            .insert(child(1, 1, vec![g[0], g[1], g[2], g[3]], 0x11))
            .unwrap();
        // Authors 2 and 3 at round 1 do NOT include r0_leader as parent —
        // they only have g[1], g[2], g[3]. So r0_leader has only 2
        // direct supporters at round 1, below q=3.
        let r1_2 = dag
            .insert(child(2, 1, vec![g[1], g[2], g[3]], 0x12))
            .unwrap();
        let r1_3 = dag
            .insert(child(3, 1, vec![g[1], g[2], g[3]], 0x13))
            .unwrap();

        // Direct rule for round 0: undecided (only 2 supporters at R+1).
        assert_eq!(try_direct_decide(&dag, 0, 4), LeaderStatus::Undecided);

        // Round 2: leader(2, 4) = 2. All 4 authors at round 2 include
        // the round-1 leader (author 1 = leader(1,4)) as parent. Direct
        // commit fires for round 2.
        let r2_0 = dag
            .insert(child(0, 2, vec![r1_0, r1_1, r1_2, r1_3], 0x20))
            .unwrap();
        let r2_1 = dag
            .insert(child(1, 2, vec![r1_0, r1_1, r1_2, r1_3], 0x21))
            .unwrap();
        let r2_2 = dag
            .insert(child(2, 2, vec![r1_0, r1_1, r1_2, r1_3], 0x22))
            .unwrap();
        let _r2_3 = dag
            .insert(child(3, 2, vec![r1_0, r1_1, r1_2, r1_3], 0x23))
            .unwrap();
        let r2_leader = r2_2; // author 2 is leader(2, 4)

        // Round 3: ≥ q=3 supporters of r2_leader.
        dag.insert(child(0, 3, vec![r2_0, r2_1, r2_2], 0x30))
            .unwrap();
        dag.insert(child(1, 3, vec![r2_0, r2_1, r2_2], 0x31))
            .unwrap();
        dag.insert(child(2, 3, vec![r2_0, r2_1, r2_2], 0x32))
            .unwrap();

        // Direct: round 2 fires.
        assert_eq!(
            try_direct_decide(&dag, 2, 4),
            LeaderStatus::Direct(r2_leader)
        );

        // Indirect: round 0 inherits from round 2 because r0_leader is
        // in the causal history of r2_leader (via r1_0 and r1_1).
        let history = causal_history(&dag, r2_leader);
        assert!(history.contains(&r0_leader));
        assert_eq!(
            decide_slot(&dag, 0, 4),
            LeaderStatus::Direct(r0_leader),
            "round 0 should inherit Direct via round-2 anchor"
        );
    }

    #[test]
    fn skipped_slot_when_not_in_anchor_history() {
        // Same setup but round 0's leader cert is NOT in any later
        // anchor's history → indirect resolves to Skip.
        let mut dag = DagStore::new();
        let mut g = Vec::new();
        for a in 0..4 {
            g.push(dag.insert(genesis(a)).unwrap());
        }
        // Authors 1, 2, 3 at round 1 omit r0_leader = g[0] entirely.
        let r1_1 = dag
            .insert(child(1, 1, vec![g[1], g[2], g[3]], 0x11))
            .unwrap();
        let r1_2 = dag
            .insert(child(2, 1, vec![g[1], g[2], g[3]], 0x12))
            .unwrap();
        let r1_3 = dag
            .insert(child(3, 1, vec![g[1], g[2], g[3]], 0x13))
            .unwrap();

        // Round 2: leader(2,4)=2. Parents do not include r0_leader.
        let r2_0 = dag
            .insert(child(0, 2, vec![r1_1, r1_2, r1_3], 0x20))
            .unwrap();
        let r2_1 = dag
            .insert(child(1, 2, vec![r1_1, r1_2, r1_3], 0x21))
            .unwrap();
        let r2_2 = dag
            .insert(child(2, 2, vec![r1_1, r1_2, r1_3], 0x22))
            .unwrap();

        // Round 3: ≥ q=3 supporters of r2_2 (= leader of round 2).
        dag.insert(child(0, 3, vec![r2_0, r2_1, r2_2], 0x30))
            .unwrap();
        dag.insert(child(1, 3, vec![r2_0, r2_1, r2_2], 0x31))
            .unwrap();
        dag.insert(child(2, 3, vec![r2_0, r2_1, r2_2], 0x32))
            .unwrap();

        // g[0] (round 0 leader cert) exists in DAG but isn't reachable
        // from r2_2 → Skip.
        assert_eq!(decide_slot(&dag, 0, 4), LeaderStatus::Skip);
    }

    #[test]
    fn direct_decide_wins_over_indirect() {
        // When a slot is directly decided, decide_slot returns Direct
        // without consulting later anchors.
        let mut dag = DagStore::new();
        let mut g = Vec::new();
        for a in 0..4 {
            g.push(dag.insert(genesis(a)).unwrap());
        }
        let leader_hash = g[0];
        for a in 0..3 {
            dag.insert(child(a, 1, vec![leader_hash], a as u8)).unwrap();
        }
        assert_eq!(
            decide_slot(&dag, 0, 4),
            LeaderStatus::Direct(leader_hash),
            "direct decision must short-circuit"
        );
    }

    // ----------------------------------------------------------------
    // Equivalence proptest — verifies the index-based substitution is
    // byte-identical to the original linearize()-based implementation.
    //
    // The reference functions below are verbatim copies of the original
    // linearize()-based bodies. They share ZERO code with the production
    // implementations. If this proptest passes, the substitution is safe.
    // ----------------------------------------------------------------
    mod equivalence {
        use super::*;
        use proptest::prelude::*;
        use std::collections::HashSet as StdHashSet;

        // ---- Reference implementations (original linearize-based bodies) ----

        fn cert_at_reference(
            dag: &DagStore,
            round: Round,
            author: AuthorityId,
        ) -> Option<CertHash> {
            dag.linearize().into_iter().find(|h| {
                let c = dag.get(h).expect("hash from linearize must resolve");
                c.round == round && c.author == author
            })
        }

        fn supporters_reference(
            dag: &DagStore,
            target: CertHash,
            round: Round,
        ) -> BTreeSet<AuthorityId> {
            let mut authors = BTreeSet::new();
            for h in dag.linearize() {
                let c = dag.get(&h).expect("hash from linearize must resolve");
                if c.round == round && c.parents.contains(&target) {
                    authors.insert(c.author);
                }
            }
            authors
        }

        fn causal_history_reference(dag: &DagStore, start: CertHash) -> Vec<CertHash> {
            let mut seen: StdHashSet<CertHash> = StdHashSet::new();
            let mut queue: VecDeque<CertHash> = VecDeque::new();
            queue.push_back(start);
            while let Some(h) = queue.pop_front() {
                if !seen.insert(h) {
                    continue;
                }
                if let Some(c) = dag.get(&h) {
                    for p in &c.parents {
                        queue.push_back(*p);
                    }
                }
            }
            dag.linearize()
                .into_iter()
                .filter(|h| seen.contains(h))
                .collect()
        }

        fn try_direct_decide_reference(
            dag: &DagStore,
            round: Round,
            n: CommitteeSize,
        ) -> LeaderStatus {
            let author = leader(round, n);
            let leader_hash = match cert_at_reference(dag, round, author) {
                Some(h) => h,
                None => return LeaderStatus::Undecided,
            };
            let support = supporters_reference(dag, leader_hash, round + 1);
            if support.len() as u32 >= quorum_threshold(n) {
                LeaderStatus::Direct(leader_hash)
            } else {
                LeaderStatus::Undecided
            }
        }

        fn try_indirect_decide_reference(
            dag: &DagStore,
            target_round: Round,
            anchor: CertHash,
            n: CommitteeSize,
        ) -> LeaderStatus {
            let target_author = leader(target_round, n);
            let target_hash = match cert_at_reference(dag, target_round, target_author) {
                Some(h) => h,
                None => return LeaderStatus::Skip,
            };
            if causal_history_reference(dag, anchor).contains(&target_hash) {
                LeaderStatus::Direct(target_hash)
            } else {
                LeaderStatus::Skip
            }
        }

        fn decide_slot_reference(
            dag: &DagStore,
            target_round: Round,
            n: CommitteeSize,
        ) -> LeaderStatus {
            match try_direct_decide_reference(dag, target_round, n) {
                LeaderStatus::Direct(h) => return LeaderStatus::Direct(h),
                LeaderStatus::Skip => return LeaderStatus::Skip,
                LeaderStatus::Undecided => {}
            }
            let max_round = match dag
                .linearize()
                .into_iter()
                .filter_map(|h| dag.get(&h).map(|c| c.round))
                .max()
            {
                Some(r) => r,
                None => return LeaderStatus::Undecided,
            };
            let mut any_anchor_seen = false;
            for anchor_round in (target_round + 2)..=max_round {
                if let LeaderStatus::Direct(anchor_hash) =
                    try_direct_decide_reference(dag, anchor_round, n)
                {
                    any_anchor_seen = true;
                    match try_indirect_decide_reference(dag, target_round, anchor_hash, n) {
                        LeaderStatus::Direct(h) => return LeaderStatus::Direct(h),
                        LeaderStatus::Skip | LeaderStatus::Undecided => continue,
                    }
                }
            }
            if any_anchor_seen {
                LeaderStatus::Skip
            } else {
                LeaderStatus::Undecided
            }
        }

        fn finalize_reference(dag: &DagStore, n: CommitteeSize) -> Vec<CertHash> {
            let max_round = match dag
                .linearize()
                .into_iter()
                .map(|h| dag.get(&h).unwrap().round)
                .max()
            {
                Some(r) => r,
                None => return Vec::new(),
            };
            let mut finalized: Vec<CertHash> = Vec::new();
            let mut included: StdHashSet<CertHash> = StdHashSet::new();
            for r in 0..max_round {
                if let LeaderStatus::Direct(leader_hash) = decide_slot_reference(dag, r, n) {
                    for h in causal_history_reference(dag, leader_hash) {
                        if included.insert(h) {
                            finalized.push(h);
                        }
                    }
                }
            }
            finalized
        }

        // ---- DAG generator ----
        //
        // Builds a random valid DAG: varied committee size, varied depth,
        // SPARSE per-author parent sets (so siblings at the same round have
        // DIFFERENT ancestor sets — this is what makes the indirect
        // Skip→Direct path reachable, where a target leader cert exists but
        // is NOT in a later anchor's causal history), potential equivocation
        // (2 certs at same (round,author)), and potential missing leaders.

        /// Pick a sparse subset of `prev` as parents for cert `(round, author)`.
        ///
        /// The per-author XOR makes two siblings at the same round select
        /// DIFFERENT parents from the same `prev`, so their causal histories
        /// diverge — the exact topology the dense `prev.clone()` generator
        /// never produced (and the reason the IQ-004 multi-anchor indirect
        /// branch now gets real differential coverage). Falls back to a single
        /// parent when the mask selects none, keeping the chain connected.
        fn select_parents(prev: &[CertHash], mask: u64, author: CommitteeSize) -> Vec<CertHash> {
            if prev.is_empty() {
                return vec![];
            }
            let seed = mask ^ (author as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            let mut parents: Vec<CertHash> = prev
                .iter()
                .copied()
                .enumerate()
                .filter(|(i, _)| seed & (1u64 << (i % 64)) != 0)
                .map(|(_, h)| h)
                .collect();
            if parents.is_empty() {
                parents.push(prev[0]);
            }
            parents
        }

        fn build_random_dag(
            n: CommitteeSize,
            n_rounds: u64,
            // per-round bitmask: which authors produce a cert (as u64 bitmask)
            author_masks: &[u64],
            // per-round equivocation mask: which authors produce a 2nd cert
            equivoc_masks: &[u64],
            // per-round bitmask: which of prev's certs are eligible parents
            // (XOR'd with author in select_parents so siblings differ)
            parent_masks: &[u64],
        ) -> DagStore {
            let mut dag = DagStore::new();
            let mut round_hashes: Vec<Vec<CertHash>> = Vec::new();

            for r in 0..n_rounds {
                let mask = author_masks[r as usize];
                let eq_mask = equivoc_masks[r as usize];
                let p_mask = parent_masks[r as usize];
                let prev = if r == 0 {
                    vec![]
                } else {
                    round_hashes[r as usize - 1].clone()
                };

                let mut this_round: Vec<CertHash> = Vec::new();
                for a in 0..n {
                    if mask & (1u64 << a) == 0 {
                        continue;
                    }
                    let parents = if r == 0 {
                        vec![]
                    } else {
                        select_parents(&prev, p_mask, a)
                    };
                    let mut payload = [0u8; 32];
                    payload[0] = a as u8;
                    payload[1] = r as u8;
                    payload[2] = 0xAA;
                    let cert = if r == 0 {
                        Certificate::genesis(a as AuthorityId, payload)
                    } else {
                        Certificate {
                            author: a as AuthorityId,
                            round: r,
                            parents: parents.clone(),
                            payload_digest: payload,
                        }
                    };
                    if let Ok(h) = dag.insert(cert) {
                        this_round.push(h);
                    }

                    // Equivocating second cert for same (round, author)
                    // (distinct payload_digest so hash differs). Shares the
                    // sibling's sparse parent set.
                    if eq_mask & (1u64 << a) != 0 {
                        let mut payload2 = [0u8; 32];
                        payload2[0] = a as u8;
                        payload2[1] = r as u8;
                        payload2[2] = 0xBB; // differs from 0xAA above
                        let cert2 = if r == 0 {
                            Certificate::genesis(a as AuthorityId, payload2)
                        } else {
                            Certificate {
                                author: a as AuthorityId,
                                round: r,
                                parents: parents.clone(),
                                payload_digest: payload2,
                            }
                        };
                        if let Ok(h2) = dag.insert(cert2) {
                            this_round.push(h2);
                        }
                    }
                }
                round_hashes.push(this_round);
            }
            dag
        }

        proptest! {
            #![proptest_config(ProptestConfig {
                cases: 4096,
                max_shrink_iters: 64,
                ..ProptestConfig::default()
            })]

            /// Equivalence oracle: the index-based production functions must
            /// produce byte-identical results to the original linearize()-based
            /// reference implementations on every random valid DAG — including
            /// DAGs with equivocation, missing leaders, and deep round chains.
            ///
            /// Covers:
            /// - `cert_at` for all (round, author) pairs
            /// - `supporters` (via `try_direct_decide` call chain)
            /// - `causal_history` for every cert in the DAG
            /// - `decide_slot` for all rounds — including the indirect
            ///   Skip→Direct path, now reachable thanks to sparse per-author
            ///   parent sets (siblings have divergent ancestor sets, so a
            ///   target leader can exist yet be absent from a later anchor's
            ///   causal history — the IQ-004 multi-anchor scan)
            /// - **`finalize`** full committed-sequence equality (headline)
            #[test]
            fn index_vs_linearize_equivalence(
                n in 1u32..=13u32,
                // Cap at 12 rounds: deep enough for multi-wave indirect
                // commit chains (target → anchor+2 → anchor+4...) while
                // keeping per-case O(R^2 × |certs|) work bounded in
                // debug mode at 4096 cases.
                n_rounds in 1u64..=12u64,
                author_masks in proptest::collection::vec(any::<u64>(), 1..=12),
                equivoc_masks in proptest::collection::vec(any::<u64>(), 1..=12),
                parent_masks in proptest::collection::vec(any::<u64>(), 1..=12),
            ) {
                // Pad/truncate masks to n_rounds length.
                let n_rounds = n_rounds.min(12);
                let mut am = author_masks;
                let mut em = equivoc_masks;
                let mut pm = parent_masks;
                am.resize(n_rounds as usize, u64::MAX);
                em.resize(n_rounds as usize, 0u64);
                // Pad parent masks with an alternating-bit pattern (sparse,
                // not full) so padded rounds still exercise divergent ancestor
                // sets rather than collapsing back to dense parents.
                pm.resize(n_rounds as usize, 0xAAAA_AAAA_AAAA_AAAA);
                // Ensure genesis round always has at least one author.
                am[0] |= 1;

                let dag = build_random_dag(n, n_rounds, &am, &em, &pm);

                if dag.is_empty() {
                    return Ok(());
                }

                // 1. cert_at equivalence for all (round, author).
                for round in dag.rounds() {
                    for author in 0..n {
                        let prod = cert_at(&dag, round, author as AuthorityId);
                        let refr = cert_at_reference(&dag, round, author as AuthorityId);
                        prop_assert_eq!(
                            prod, refr,
                            "cert_at diverged at round={} author={}",
                            round, author
                        );
                    }
                }

                // 2. decide_slot equivalence for all rounds.
                let max_r = dag.max_round().unwrap_or(0);
                for round in 0..=max_r {
                    let prod = decide_slot(&dag, round, n);
                    let refr = decide_slot_reference(&dag, round, n);
                    prop_assert_eq!(
                        prod, refr,
                        "decide_slot diverged at round={} n={}",
                        round, n
                    );
                }

                // 3. causal_history equivalence for every cert in the DAG.
                for round in dag.rounds() {
                    for &h in dag.round_hashes(round) {
                        let prod = causal_history(&dag, h);
                        let refr = causal_history_reference(&dag, h);
                        prop_assert_eq!(
                            prod, refr,
                            "causal_history diverged starting from hash in round={}",
                            round
                        );
                    }
                }

                // 4. finalize equivalence (headline assertion).
                let prod_fin = finalize(&dag, n);
                let ref_fin = finalize_reference(&dag, n);
                prop_assert_eq!(
                    prod_fin, ref_fin,
                    "finalize diverged: committed order differs for n={} n_rounds={}",
                    n, n_rounds
                );
            }
        }
    }
}
