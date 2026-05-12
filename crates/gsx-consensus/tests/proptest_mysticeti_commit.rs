//! DAG-S4 exit-gate property tests.
//!
//! Exit gate: `mysticeti_c_finality` — once a leader is committed, no DAG
//! extension can uncommit it. This is the load-bearing finality property
//! of the paper's §6.2 commit rule.
//!
//! Supporting properties:
//!
//! - `commit_requires_quorum` — a leader without `quorum_threshold(n)`
//!   distinct supporters at the next round is not committed.
//! - `leaders_commit_deterministically` — the commit decision is a pure
//!   function of the DAG.
//! - `finalize_is_append_only` — extending the DAG by more certificates
//!   can only append to the finalized prefix, never reorder or remove.
//!
//! Run at default 256 cases under CI; sprint close runs
//! `PROPTEST_CASES=10000 cargo test -p gsx-consensus --release`.

use gsx_consensus::{
    commit_leader, finalize, leader, quorum_threshold, AuthorityId, CertHash, Certificate,
    CommitteeSize, DagStore, Round,
};
use proptest::prelude::*;

/// Build a valid dense DAG of `n_rounds` rounds with `n_authorities`
/// authors per round, where each non-genesis cert references every
/// round-(r-1) cert as parents. Returns the certs in topo-order.
fn build_dense_dag(
    n_rounds: u64,
    n_authorities: CommitteeSize,
    payload_seed: u64,
) -> Vec<Certificate> {
    let mut all = Vec::new();
    let mut prev_round_hashes: Vec<CertHash> = Vec::new();

    for r in 0..n_rounds {
        let mut this_round = Vec::with_capacity(n_authorities as usize);
        for a in 0..n_authorities {
            let mut payload = [0u8; 32];
            payload[0] = a as u8;
            payload[1] = r as u8;
            payload[2] = (payload_seed & 0xFF) as u8;
            let cert = if r == 0 {
                Certificate::genesis(a as AuthorityId, payload)
            } else {
                Certificate {
                    author: a as AuthorityId,
                    round: r as Round,
                    parents: prev_round_hashes.clone(),
                    payload_digest: payload,
                }
            };
            this_round.push(cert.hash());
            all.push(cert);
        }
        prev_round_hashes = this_round;
    }
    all
}

/// Insert a topo-ordered cert list into a fresh `DagStore`.
fn store_from(certs: &[Certificate]) -> DagStore {
    let mut s = DagStore::new();
    for c in certs {
        s.insert(c.clone())
            .expect("topo-ordered insert must succeed");
    }
    s
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 32,
        .. ProptestConfig::default()
    })]

    /// EXIT GATE — once a leader at round `r` is committed in a DAG `D`,
    /// it remains committed in every extension `D' ⊇ D`: adding more
    /// certificates at later rounds, or more witnesses at round `r+1`,
    /// only grows the supporter set. The commit decision is monotone.
    #[test]
    fn mysticeti_c_finality(
        n_authorities in 1u32..=8,
        n_rounds in 2u64..=6,
        payload_seed in any::<u64>(),
        extra_round_rounds in 0u64..=4,
    ) {
        // Build the base DAG and identify any rounds where the leader
        // is committed.
        let base = build_dense_dag(n_rounds, n_authorities, payload_seed);
        let store_base = store_from(&base);

        let mut committed_rounds: Vec<(Round, CertHash)> = Vec::new();
        for r in 0..n_rounds.saturating_sub(1) {
            if let Some(h) = commit_leader(&store_base, r, n_authorities) {
                committed_rounds.push((r, h));
            }
        }

        // Extend the DAG by `extra_round_rounds` additional rounds, same
        // dense pattern. The extension is monotonic by construction.
        let total_rounds = n_rounds + extra_round_rounds;
        let extended = build_dense_dag(total_rounds, n_authorities, payload_seed);
        let store_ext = store_from(&extended);

        for (r, leader_hash) in committed_rounds {
            let leader_in_ext = commit_leader(&store_ext, r, n_authorities);
            prop_assert_eq!(
                leader_in_ext,
                Some(leader_hash),
                "finality violated: leader at round {} uncommitted by extension",
                r,
            );
        }
    }

    /// A leader without `quorum_threshold(n)` distinct supporters at the
    /// next round is not committed.
    #[test]
    fn commit_requires_quorum(
        n_authorities in 2u32..=8,
        payload_seed in any::<u64>(),
    ) {
        let q = quorum_threshold(n_authorities);

        // Build round-0 genesis certs for every authority.
        let mut dag = DagStore::new();
        let mut genesis_hashes = Vec::new();
        for a in 0..n_authorities {
            let mut p = [0u8; 32];
            p[0] = a as u8;
            p[1] = (payload_seed & 0xFF) as u8;
            let cert = Certificate::genesis(a as AuthorityId, p);
            genesis_hashes.push(cert.hash());
            dag.insert(cert).unwrap();
        }

        let leader_author = leader(0, n_authorities);
        let leader_hash = genesis_hashes[leader_author as usize];

        // Add exactly `q - 1` supporters at round 1 → must NOT commit.
        for a in 0..(q - 1) {
            let mut p = [0u8; 32];
            p[0] = a as u8;
            p[1] = 0xFF;
            dag.insert(Certificate {
                author: a as AuthorityId,
                round: 1,
                parents: vec![leader_hash],
                payload_digest: p,
            })
                .unwrap();
        }
        prop_assert_eq!(commit_leader(&dag, 0, n_authorities), None);
    }

    /// Commit decisions are a pure function of the DAG: two stores with
    /// the same certificate set produce the same commit decisions.
    #[test]
    fn leaders_commit_deterministically(
        n_authorities in 1u32..=8,
        n_rounds in 1u64..=5,
        payload_seed in any::<u64>(),
    ) {
        let base = build_dense_dag(n_rounds, n_authorities, payload_seed);
        let store_a = store_from(&base);
        // Same set, but reverse the insertion order within each round.
        let mut by_round: std::collections::BTreeMap<Round, Vec<Certificate>> =
            Default::default();
        for c in base {
            by_round.entry(c.round).or_default().push(c);
        }
        let mut store_b = DagStore::new();
        for (_r, mut group) in by_round {
            group.reverse();
            for c in group {
                store_b.insert(c).unwrap();
            }
        }

        for r in 0..n_rounds.saturating_sub(1) {
            prop_assert_eq!(
                commit_leader(&store_a, r, n_authorities),
                commit_leader(&store_b, r, n_authorities),
            );
        }
    }

    /// `finalize(D)` is a prefix of `finalize(D')` for any extension
    /// `D' ⊇ D`: extending the DAG can only append to the finalized
    /// commit sequence.
    #[test]
    fn finalize_is_append_only(
        n_authorities in 1u32..=6,
        n_rounds in 1u64..=4,
        payload_seed in any::<u64>(),
        extra_rounds in 0u64..=3,
    ) {
        let base = build_dense_dag(n_rounds, n_authorities, payload_seed);
        let ext = build_dense_dag(n_rounds + extra_rounds, n_authorities, payload_seed);

        let store_base = store_from(&base);
        let store_ext = store_from(&ext);

        let fin_base = finalize(&store_base, n_authorities);
        let fin_ext = finalize(&store_ext, n_authorities);

        prop_assert!(
            fin_ext.len() >= fin_base.len(),
            "extension cannot shrink finalized prefix",
        );
        for (i, h) in fin_base.iter().enumerate() {
            prop_assert_eq!(
                fin_ext[i], *h,
                "extension reordered finalized cert at index {}",
                i,
            );
        }
    }
}
