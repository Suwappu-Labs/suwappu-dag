//! Mysticeti-C commit rule over the certificate DAG.
//!
//! Implements §6.2 of the *GSX DAG Layer 1* paper: deterministic finality
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
pub fn cert_at(dag: &DagStore, round: Round, author: AuthorityId) -> Option<CertHash> {
    dag.linearize().into_iter().find(|h| {
        let c = dag.get(h).expect("hash from linearize must resolve");
        c.round == round && c.author == author
    })
}

/// Return the set of distinct authors at `round` who directly reference
/// `target` as one of their parents.
fn supporters(dag: &DagStore, target: CertHash, round: Round) -> BTreeSet<AuthorityId> {
    let mut authors = BTreeSet::new();
    for h in dag.linearize() {
        let c = dag.get(&h).expect("hash from linearize must resolve");
        if c.round == round && c.parents.contains(&target) {
            authors.insert(c.author);
        }
    }
    authors
}

/// Determine whether the leader at `round` is committed under the
/// Mysticeti-C rule for an Authority Ring of size `n`.
///
/// Returns `Some(leader_hash)` if committed, `None` otherwise. A leader
/// can fail to commit because (i) the leader did not produce a
/// certificate at `round`, or (ii) fewer than `quorum_threshold(n)`
/// supporters exist at `round + 1`.
pub fn commit_leader(dag: &DagStore, round: Round, n: CommitteeSize) -> Option<CertHash> {
    let author = leader(round, n);
    let leader_hash = cert_at(dag, round, author)?;
    let support = supporters(dag, leader_hash, round + 1);
    if support.len() as u32 >= quorum_threshold(n) {
        Some(leader_hash)
    } else {
        None
    }
}

/// Walk the DAG from `start` and collect every ancestor (including
/// `start` itself). Returns the ancestor set as a deterministic
/// linearized vector following `DagStore::linearize()`'s order.
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
    dag.linearize()
        .into_iter()
        .filter(|h| seen.contains(h))
        .collect()
}

/// Run the Mysticeti-C commit rule across every round of the DAG and
/// return the finalized linear history.
///
/// The output is the deterministic concatenation of causal histories of
/// the committed leaders, deduplicated to preserve append-only finality.
pub fn finalize(dag: &DagStore, n: CommitteeSize) -> Vec<CertHash> {
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
    let mut included: HashSet<CertHash> = HashSet::new();

    // Iterate rounds in ascending order. The commit rule needs round
    // r+1 to evaluate the leader at round r, so we cannot evaluate the
    // top round.
    for r in 0..max_round {
        if let Some(leader_hash) = commit_leader(dag, r, n) {
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
            assert!(
                3 * q > 2 * n,
                "n={n}: q={q} fails strict 2/3 majority"
            );
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
}
