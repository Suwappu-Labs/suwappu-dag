//! DAG-S3 exit-gate property tests.
//!
//! Exit gate: `dag_topological_order_unique` — the linearization of any
//! valid DAG is a deterministic function of the certificate set, not of
//! insertion order.
//!
//! Supporting properties:
//!
//! - `linearization_respects_parent_order` — for every (parent, child)
//!   edge, the parent appears before the child in the linearization.
//! - `linearization_is_complete` — every inserted certificate appears in
//!   the linearization exactly once.
//! - `linearization_groups_by_round` — within the linearization, rounds
//!   appear in non-decreasing order.
//!
//! Run at default 256 cases under CI; sprint close runs
//! `PROPTEST_CASES=10000 cargo test -p suwappu-consensus --release`.

use std::collections::HashSet;

use suwappu_consensus::{AuthorityId, CertHash, Certificate, DagStore, Round};
use proptest::prelude::*;
use rand::{rngs::StdRng, seq::SliceRandom, SeedableRng};

/// Build a valid random DAG: `n_rounds` rounds, `n_authorities` authors
/// per round. Each non-genesis certificate references *all* certificates
/// of the previous round as parents (a fully-connected DAG, the densest
/// valid topology; sparser topologies are a subset of this and inherit
/// the determinism property).
fn build_dag(
    n_rounds: u64,
    n_authorities: u32,
    payload_seed: u64,
) -> (Vec<Certificate>, Vec<Vec<CertHash>>) {
    let mut all_certs = Vec::new();
    // hashes_by_round[r] = vec of cert hashes at round r
    let mut hashes_by_round: Vec<Vec<CertHash>> = Vec::with_capacity(n_rounds as usize);

    // Round 0: genesis per authority.
    let mut round_0 = Vec::with_capacity(n_authorities as usize);
    for a in 0..n_authorities {
        let payload = {
            let mut p = [0u8; 32];
            p[0] = a as u8;
            p[1] = (payload_seed & 0xFF) as u8;
            p
        };
        let cert = Certificate::genesis(a as AuthorityId, payload);
        round_0.push(cert.hash());
        all_certs.push(cert);
    }
    hashes_by_round.push(round_0);

    // Rounds 1..n_rounds: each authority's cert references all previous
    // round's certs as parents.
    for r in 1..n_rounds {
        let mut this_round = Vec::with_capacity(n_authorities as usize);
        let parents = hashes_by_round[(r - 1) as usize].clone();
        for a in 0..n_authorities {
            let payload = {
                let mut p = [0u8; 32];
                p[0] = a as u8;
                p[1] = r as u8;
                p[2] = (payload_seed & 0xFF) as u8;
                p
            };
            let cert = Certificate {
                author: a as AuthorityId,
                round: r as Round,
                parents: parents.clone(),
                payload_digest: payload,
            };
            this_round.push(cert.hash());
            all_certs.push(cert);
        }
        hashes_by_round.push(this_round);
    }

    (all_certs, hashes_by_round)
}

/// Topologically-sort a flat list of certificates: parents must come
/// before children. The build_dag function already returns them in such
/// an order, but a shuffle may violate it, so we recompute here.
fn topo_sorted(certs: Vec<Certificate>, seed: u64) -> Vec<Certificate> {
    // Sort by round first (parents always have smaller round), then by a
    // seeded shuffle within each round. This is a valid insertion order.
    let mut rng = StdRng::seed_from_u64(seed);
    let mut by_round: std::collections::BTreeMap<Round, Vec<Certificate>> =
        std::collections::BTreeMap::new();
    for c in certs {
        by_round.entry(c.round).or_default().push(c);
    }
    let mut out = Vec::new();
    for (_r, mut group) in by_round {
        group.shuffle(&mut rng);
        out.extend(group);
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 32,
        .. ProptestConfig::default()
    })]

    /// Exit gate: linearization is a function of the certificate set,
    /// not of insertion order. Two stores built from the same certificates
    /// in different orders produce identical linearizations.
    #[test]
    fn dag_topological_order_unique(
        n_rounds in 1u64..=6,
        n_authorities in 1u32..=8,
        payload_seed in any::<u64>(),
        shuffle_seed_a in any::<u64>(),
        shuffle_seed_b in any::<u64>(),
    ) {
        let (certs, _hashes) = build_dag(n_rounds, n_authorities, payload_seed);
        let insertion_a = topo_sorted(certs.clone(), shuffle_seed_a);
        let insertion_b = topo_sorted(certs, shuffle_seed_b);

        let mut store_a = DagStore::new();
        for c in insertion_a {
            store_a.insert(c).expect("valid topo insertion must succeed");
        }
        let mut store_b = DagStore::new();
        for c in insertion_b {
            store_b.insert(c).expect("valid topo insertion must succeed");
        }

        prop_assert_eq!(store_a.linearize(), store_b.linearize());
    }

    /// Linearization respects the parent-child DAG edges: for every cert
    /// `c` with parent `p`, `p` appears before `c` in the linearization.
    #[test]
    fn linearization_respects_parent_order(
        n_rounds in 1u64..=5,
        n_authorities in 1u32..=6,
        payload_seed in any::<u64>(),
        insert_seed in any::<u64>(),
    ) {
        let (certs, _) = build_dag(n_rounds, n_authorities, payload_seed);
        let insertion = topo_sorted(certs.clone(), insert_seed);
        let mut store = DagStore::new();
        for c in insertion {
            store.insert(c).unwrap();
        }

        let order = store.linearize();
        let position: std::collections::HashMap<CertHash, usize> = order
            .iter()
            .enumerate()
            .map(|(i, h)| (*h, i))
            .collect();

        for cert in &certs {
            let child_pos = position[&cert.hash()];
            for parent_hash in &cert.parents {
                let parent_pos = position[parent_hash];
                prop_assert!(
                    parent_pos < child_pos,
                    "parent must precede child in linearization",
                );
            }
        }
    }

    /// Every inserted certificate appears in the linearization exactly
    /// once.
    #[test]
    fn linearization_is_complete(
        n_rounds in 1u64..=5,
        n_authorities in 1u32..=6,
        payload_seed in any::<u64>(),
        insert_seed in any::<u64>(),
    ) {
        let (certs, _) = build_dag(n_rounds, n_authorities, payload_seed);
        let insertion = topo_sorted(certs.clone(), insert_seed);
        let mut store = DagStore::new();
        for c in insertion {
            store.insert(c).unwrap();
        }

        let order = store.linearize();
        prop_assert_eq!(order.len(), certs.len());

        let order_set: HashSet<CertHash> = order.into_iter().collect();
        let expected: HashSet<CertHash> = certs.iter().map(|c| c.hash()).collect();
        prop_assert_eq!(order_set, expected);
    }

    /// Within the linearization, rounds appear in non-decreasing order.
    #[test]
    fn linearization_groups_by_round(
        n_rounds in 1u64..=5,
        n_authorities in 1u32..=6,
        payload_seed in any::<u64>(),
        insert_seed in any::<u64>(),
    ) {
        let (certs, _) = build_dag(n_rounds, n_authorities, payload_seed);
        let insertion = topo_sorted(certs.clone(), insert_seed);
        let mut store = DagStore::new();
        for c in insertion {
            store.insert(c).unwrap();
        }

        let order = store.linearize();
        let rounds: Vec<Round> = order
            .iter()
            .map(|h| store.get(h).unwrap().round)
            .collect();
        for window in rounds.windows(2) {
            prop_assert!(window[0] <= window[1]);
        }
    }
}
