//! Perf-loop gate: the bounded reachability search the indirect rule uses
//! is exactly membership in the causal history.
//!
//! - `reaches_equals_causal_history_membership` — over sparse random DAGs
//!   (random parent subsets, omitted slots, optional pruning below a gc
//!   round), `reaches(dag, a, t) == causal_history(dag, a).contains(&t)`
//!   for every pair of held certificates.
//! - `indirect_rule_unchanged` — `decide_slot_for` over the same DAGs
//!   equals a reference that uses the full causal history, for every slot.
//! - `history_order_matches_lookup_sort` — `causal_history` and
//!   `causal_history_bounded` return exactly the (round, author, hash)
//!   order of a comparator-time-lookup sort (the pre-perf-loop code).
//!
//! Sprint-close run:
//! `PROPTEST_CASES=10000 cargo test -p suwappu-consensus --release --test proptest_reachability`.

use proptest::prelude::*;
use suwappu_consensus::{
    causal_history, causal_history_bounded, cert_at, decide_slot_for, reaches,
    try_direct_decide_for, CertHash, Certificate, Committee, DagStore, LeaderStatus, Round,
};

/// `n` authors, `rounds` rounds. Each slot is omitted with its omit bit;
/// each present certificate references the previous round's present
/// certificates selected by its parent mask (at least one when any
/// exists, so the DAG stays connected enough to be interesting).
fn build_sparse(n: usize, rounds: u64, omit: u64, masks: &[u8], seed: u8) -> Vec<Certificate> {
    let mut certs = Vec::new();
    let mut prev: Vec<CertHash> = Vec::new();
    let mut prev2: Vec<CertHash> = Vec::new();
    let mut k = 0usize;
    for r in 0..rounds {
        let mut this = Vec::new();
        for i in 0..n {
            let bit = (r as usize * n + i) % 64;
            if r > 0 && (omit >> bit) & 1 == 1 {
                continue;
            }
            let mask = masks[k % masks.len()];
            k += 1;
            let mut parents: Vec<CertHash> = prev
                .iter()
                .enumerate()
                .filter(|(j, _)| (mask >> (j % 8)) & 1 == 1)
                .map(|(_, h)| *h)
                .collect();
            if parents.is_empty() && !prev.is_empty() {
                parents.push(prev[(mask as usize) % prev.len()]);
            }
            // A round-skipping edge (a Byzantine author may reference any
            // lower round): exercises paths that bypass round r + 1.
            if (mask >> 7) & 1 == 1 && !prev2.is_empty() {
                let p = prev2[(mask as usize / 3) % prev2.len()];
                if !parents.contains(&p) {
                    parents.push(p);
                }
            }
            let mut payload = [0u8; 32];
            payload[0] = i as u8;
            payload[1] = r as u8;
            payload[2] = seed;
            let cert = Certificate {
                author: i as u32,
                round: r,
                parents,
                payload_digest: payload,
                signature: Vec::new(),
            };
            this.push(cert.hash());
            certs.push(cert);
        }
        prev2 = std::mem::replace(&mut prev, this);
    }
    certs
}

fn store(certs: &[Certificate], prune: Option<Round>) -> DagStore {
    let mut dag = DagStore::new();
    for c in certs {
        dag.insert(c.clone()).expect("topological insert");
    }
    if let Some(g) = prune {
        dag.prune_below(g);
    }
    dag
}

/// The indirect rule as it was: full causal history, then membership.
fn reference_decide(dag: &DagStore, target_round: Round, committee: &Committee) -> LeaderStatus {
    match try_direct_decide_for(dag, target_round, committee) {
        LeaderStatus::Direct(h) => return LeaderStatus::Direct(h),
        LeaderStatus::Skip => return LeaderStatus::Skip,
        LeaderStatus::Undecided => {}
    }
    let Some(max_round) = dag.max_round() else {
        return LeaderStatus::Undecided;
    };
    let mut saw_anchor = false;
    for anchor_round in (target_round + 2)..=max_round {
        if let LeaderStatus::Direct(anchor) = try_direct_decide_for(dag, anchor_round, committee) {
            saw_anchor = true;
            let Some(author) = committee.leader(target_round) else {
                continue;
            };
            let Some(target) = cert_at(dag, target_round, author) else {
                continue;
            };
            if causal_history(dag, anchor).contains(&target) {
                return LeaderStatus::Direct(target);
            }
        }
    }
    if saw_anchor {
        LeaderStatus::Skip
    } else {
        LeaderStatus::Undecided
    }
}

proptest! {
    #[test]
    fn reaches_equals_causal_history_membership(
        n in 4usize..=7,
        rounds in 2u64..=12,
        omit in any::<u64>(),
        masks in proptest::collection::vec(any::<u8>(), 1..32),
        seed in any::<u8>(),
        prune in proptest::option::of(0u64..6),
    ) {
        let certs = build_sparse(n, rounds, omit, &masks, seed);
        let dag = store(&certs, prune);
        let held: Vec<CertHash> = certs.iter().map(|c| c.hash()).collect();
        for a in &held {
            let history = causal_history(&dag, *a);
            for t in &held {
                prop_assert_eq!(reaches(&dag, *a, *t), history.contains(t));
            }
        }
    }

    #[test]
    fn indirect_rule_unchanged(
        n in 4usize..=7,
        rounds in 3u64..=12,
        omit in any::<u64>(),
        masks in proptest::collection::vec(any::<u8>(), 1..32),
        seed in any::<u8>(),
        prune in proptest::option::of(0u64..4),
    ) {
        let certs = build_sparse(n, rounds, omit, &masks, seed);
        let dag = store(&certs, prune);
        let committee = Committee::contiguous(n as u32);
        for r in 0..rounds {
            prop_assert_eq!(
                decide_slot_for(&dag, r, &committee),
                reference_decide(&dag, r, &committee),
                "slot {}", r
            );
        }
    }

    #[test]
    fn history_order_matches_lookup_sort(
        n in 4usize..=7,
        rounds in 2u64..=12,
        omit in any::<u64>(),
        masks in proptest::collection::vec(any::<u8>(), 1..32),
        seed in any::<u8>(),
        prune in proptest::option::of(0u64..6),
        floor in proptest::option::of(0u64..8),
    ) {
        let certs = build_sparse(n, rounds, omit, &masks, seed);
        let dag = store(&certs, prune);
        let reference = |mut v: Vec<CertHash>| {
            v.sort_by_key(|h| {
                let c = dag.get(h).expect("held");
                (c.round, c.author, *h)
            });
            v
        };
        for c in &certs {
            let a = c.hash();
            let full = causal_history(&dag, a);
            prop_assert_eq!(&full, &reference(full.clone()));
            let bounded = causal_history_bounded(&dag, a, floor);
            prop_assert_eq!(&bounded, &reference(bounded.clone()));
            let expected: Vec<CertHash> = full
                .iter()
                .copied()
                .filter(|h| floor.map_or(true, |f| dag.get(h).expect("held").round > f))
                .collect();
            prop_assert_eq!(bounded, expected);
        }
    }
}
