//! DAG-S34.1 exit-gate property tests (IQ-008 D1/D2).
//!
//! Garbage collection must be *invisible* to the commit rule for every
//! round it retains, and *bounded* for every round it evicts:
//!
//! - `decide_slot_is_prune_invariant` (I-GC2) — pruning rounds at or
//!   below `g` changes no slot decision for any round above `g`. Holds
//!   because every path from an anchor down to a target leader passes
//!   only through rounds above the target (parents have strictly smaller
//!   rounds), so pruned certificates are never on such a path.
//! - `bounded_history_is_prune_invariant` (I-GC1) — the committed sub-DAG
//!   of any leader, cut at a floor at or above `g`, is identical on a
//!   pruned and an unpruned store, and equals the unbounded history
//!   filtered by round. This is the property that makes the commit floor
//!   a pure function of the leader rather than of local pruning progress.
//! - `store_size_is_bounded` (I-GC3) — after pruning, no certificate at or
//!   below `g` remains, no round index is left empty (the `max_round`
//!   invariant the commit path leans on), and tombstones stay inside one
//!   `GC_DEPTH` window.
//! - `insert_verdicts_match_after_prune` — a certificate just above the
//!   gc round validates against tombstoned parents exactly as it would
//!   have against the live ones; obsolete and orphan certificates are
//!   rejected with the documented errors.
//! - `linearize_is_prefix_stable` — pruning only removes a prefix of the
//!   linearization.
//!
//! Run at default 256 cases under CI; sprint close runs
//! `PROPTEST_CASES=10000 cargo test -p suwappu-consensus --release`.

use proptest::prelude::*;
use suwappu_consensus::{
    causal_history, causal_history_bounded, decide_slot, AuthorityId, CertHash, Certificate,
    CommitteeSize, ConsensusError, DagStore, Round, GC_DEPTH,
};

/// A random *valid* DAG: `n_rounds` rounds of up to `n_authorities`
/// certificates each. Every non-genesis certificate takes a non-empty
/// random subset of the previous round as parents, and each author is
/// independently absent from a round with the given probability, so the
/// generator covers sparse rounds, skipped leaders and partial support.
/// Returned in topological (round) order.
fn random_dag(
    n_authorities: CommitteeSize,
    n_rounds: u64,
    omit_bits: u64,
    parent_bits: u64,
) -> Vec<Certificate> {
    let mut all = Vec::new();
    let mut prev: Vec<CertHash> = Vec::new();
    let mut bit = 0u32;
    let mut next_bit = |src: u64| {
        let b = (src >> (bit % 64)) & 1 == 1;
        bit = bit.wrapping_add(1);
        b
    };
    for r in 0..n_rounds {
        let mut this = Vec::new();
        for a in 0..n_authorities {
            // Genesis always complete so round 1 has parents to choose from.
            if r > 0 && next_bit(omit_bits) {
                continue;
            }
            let parents: Vec<CertHash> = if r == 0 {
                Vec::new()
            } else {
                let mut ps: Vec<CertHash> = prev
                    .iter()
                    .copied()
                    .filter(|_| next_bit(parent_bits))
                    .collect();
                if ps.is_empty() {
                    ps.push(prev[(a as usize) % prev.len()]);
                }
                ps
            };
            let mut payload = [0u8; 32];
            payload[0] = a as u8;
            payload[1] = r as u8;
            payload[2] = (omit_bits & 0xFF) as u8;
            payload[3] = (parent_bits & 0xFF) as u8;
            let cert = Certificate {
                author: a as AuthorityId,
                round: r as Round,
                parents,
                payload_digest: payload,
                signature: Vec::new(),
            };
            this.push(cert.hash());
            all.push(cert);
        }
        // A round with no certificates would leave later rounds
        // parentless; fall back to keeping one author.
        if this.is_empty() {
            let mut payload = [0u8; 32];
            payload[0] = 0xF0;
            payload[1] = r as u8;
            let cert = Certificate {
                author: 0,
                round: r as Round,
                parents: vec![prev[0]],
                payload_digest: payload,
                signature: Vec::new(),
            };
            this.push(cert.hash());
            all.push(cert);
        }
        prev = this;
    }
    all
}

fn store_from(certs: &[Certificate]) -> DagStore {
    let mut s = DagStore::new();
    for c in certs {
        s.insert(c.clone())
            .expect("topo-ordered insert must succeed");
    }
    s
}

fn round_of(dag: &DagStore, h: &CertHash) -> Round {
    dag.get(h).expect("in-dag").round
}

proptest! {
    #![proptest_config(ProptestConfig {
        // Case count comes from PROPTEST_CASES (default 256); the sprint
        // exit gate runs 10,000 via scripts/check-10k.sh.
        cases: std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(256),
        max_shrink_iters: 32,
        .. ProptestConfig::default()
    })]

    /// EXIT GATE (I-GC2): pruning at `g` leaves `decide_slot` unchanged
    /// for every round above `g`.
    #[test]
    fn decide_slot_is_prune_invariant(
        n_authorities in 1u32..=7,
        n_rounds in 3u64..=12,
        omit_bits in any::<u64>(),
        parent_bits in any::<u64>(),
        g_frac in 0u64..=100,
    ) {
        let certs = random_dag(n_authorities, n_rounds, omit_bits, parent_bits);
        let orig = store_from(&certs);
        let max_round = orig.max_round().unwrap();
        let g = (max_round.saturating_sub(1)) * g_frac / 100;

        let mut pruned = orig.clone();
        pruned.prune_below(g);

        for r in (g + 1)..=max_round {
            prop_assert_eq!(
                decide_slot(&pruned, r, n_authorities),
                decide_slot(&orig, r, n_authorities),
                "decision for round {} changed by pruning at {}", r, g
            );
        }
    }

    /// EXIT GATE (I-GC1): the committed sub-DAG of any leader, cut at a
    /// floor at or above the gc round, is the same on a pruned store, on
    /// the unpruned store, and equals the unbounded history filtered by
    /// round.
    #[test]
    fn bounded_history_is_prune_invariant(
        n_authorities in 1u32..=7,
        n_rounds in 3u64..=12,
        omit_bits in any::<u64>(),
        parent_bits in any::<u64>(),
        g_frac in 0u64..=100,
        leader_pick in any::<u32>(),
        floor_extra in 0u64..=3,
    ) {
        let certs = random_dag(n_authorities, n_rounds, omit_bits, parent_bits);
        let orig = store_from(&certs);
        let max_round = orig.max_round().unwrap();
        let g = (max_round.saturating_sub(1)) * g_frac / 100;
        let mut pruned = orig.clone();
        pruned.prune_below(g);

        // Any retained certificate can play the leader.
        let candidates: Vec<CertHash> = orig
            .linearize()
            .into_iter()
            .filter(|h| round_of(&orig, h) > g)
            .collect();
        prop_assume!(!candidates.is_empty());
        let leader = candidates[(leader_pick as usize) % candidates.len()];

        // Floor at or above g — the I-GC1 precondition. (Pruning at g and
        // sweeping with no floor is exactly the divergence the floor
        // exists to prevent, so it is not a valid input here.)
        let floor = Some(g + floor_extra);

        // No-floor walk on the unpruned store is the unbounded history.
        prop_assert_eq!(
            causal_history_bounded(&orig, leader, None),
            causal_history(&orig, leader)
        );

        let on_pruned = causal_history_bounded(&pruned, leader, floor);
        let on_orig = causal_history_bounded(&orig, leader, floor);
        let reference: Vec<CertHash> = causal_history(&orig, leader)
            .into_iter()
            .filter(|h| match floor {
                Some(f) => round_of(&orig, h) > f,
                None => true,
            })
            .collect();

        prop_assert_eq!(&on_pruned, &on_orig, "pruned vs unpruned differ");
        prop_assert_eq!(&on_orig, &reference, "bounded walk vs filtered reference differ");
        // Every emitted cert is above the floor and above the gc round.
        for h in &on_pruned {
            let r = round_of(&pruned, h);
            prop_assert!(r > g);
            if let Some(f) = floor { prop_assert!(r > f); }
        }
    }

    /// EXIT GATE (I-GC1, late-flip input): a leader whose commit floor is
    /// BELOW the gc round — the IQ-004 late flip the daemon handles under
    /// `gc_late_flip` — sweeps, on a pruned store, exactly the sub-DAG the
    /// unpruned store sweeps with the floor clamped to the gc round. The
    /// daemon makes that clamp explicit; this property is what makes the
    /// clamp a statement about the DAG rather than about one node's
    /// pruning progress (consensus-review finding on S34).
    #[test]
    fn bounded_history_below_gc_is_clamped(
        n_authorities in 1u32..=7,
        n_rounds in 3u64..=12,
        omit_bits in any::<u64>(),
        parent_bits in any::<u64>(),
        g_frac in 1u64..=100,
        leader_pick in any::<u32>(),
        floor_below in 0u64..=4,
        no_floor in any::<bool>(),
    ) {
        let certs = random_dag(n_authorities, n_rounds, omit_bits, parent_bits);
        let orig = store_from(&certs);
        let max_round = orig.max_round().unwrap();
        let g = (max_round.saturating_sub(1)) * g_frac / 100;
        prop_assume!(g > 0);
        let mut pruned = orig.clone();
        pruned.prune_below(g);

        let candidates: Vec<CertHash> = orig
            .linearize()
            .into_iter()
            .filter(|h| round_of(&orig, h) > g)
            .collect();
        prop_assume!(!candidates.is_empty());
        let leader = candidates[(leader_pick as usize) % candidates.len()];

        // A floor strictly below g, or none at all.
        let floor = if no_floor { None } else { Some(g.saturating_sub(1 + floor_below)) };
        if let Some(f) = floor { prop_assume!(f < g); }

        let on_pruned = causal_history_bounded(&pruned, leader, floor);
        let clamped = causal_history_bounded(&orig, leader, Some(g));
        let reference: Vec<CertHash> = causal_history(&orig, leader)
            .into_iter()
            .filter(|h| round_of(&orig, h) > g)
            .collect();
        prop_assert_eq!(&on_pruned, &clamped, "pruned walk below gc vs clamped unpruned walk differ");
        prop_assert_eq!(&clamped, &reference, "clamped walk vs filtered reference differ");
        // And the unclamped walk on the unpruned store is a superset: the
        // divergence class IQ-008 Residual 1 accepts is exactly
        // `on_orig \ on_pruned`, all of it at or below g.
        let on_orig = causal_history_bounded(&orig, leader, floor);
        for h in &on_orig {
            if !on_pruned.contains(h) {
                prop_assert!(round_of(&orig, h) <= g);
            }
        }
    }

    /// EXIT GATE (I-GC3): after pruning nothing at or below `g` remains,
    /// no round index is empty, and tombstones are confined to one
    /// `GC_DEPTH` window below `g`.
    #[test]
    fn store_size_is_bounded(
        n_authorities in 1u32..=7,
        n_rounds in 2u64..=12,
        omit_bits in any::<u64>(),
        parent_bits in any::<u64>(),
        g_frac in 0u64..=100,
    ) {
        let certs = random_dag(n_authorities, n_rounds, omit_bits, parent_bits);
        let mut dag = store_from(&certs);
        let max_round = dag.max_round().unwrap();
        let g = (max_round.saturating_sub(1)) * g_frac / 100;
        let before = dag.len();

        let rep = dag.prune_below(g);
        let pruned_count = certs.iter().filter(|c| c.round <= g).count();
        prop_assert_eq!(rep.certs_pruned, pruned_count);
        prop_assert_eq!(dag.len(), before - pruned_count);
        prop_assert_eq!(dag.gc_round(), Some(g));

        // No obsolete certificate survives.
        for h in dag.linearize() {
            prop_assert!(round_of(&dag, &h) > g);
        }
        // Bound: at most n certs per retained round.
        prop_assert!(dag.len() as u64 <= n_authorities as u64 * (max_round - g));
        // Non-empty-round invariant that `max_round`/`rounds` rely on.
        for r in dag.rounds() {
            prop_assert!(!dag.round_hashes(r).is_empty(), "empty round index at {}", r);
            prop_assert!(r > g);
        }
        prop_assert_eq!(dag.max_round(), Some(max_round));
        // Tombstones: exactly the pruned certs, all inside the window.
        prop_assert_eq!(dag.tombstone_count(), pruned_count);
        prop_assert!(dag.tombstone_count() as u64 <= n_authorities as u64 * GC_DEPTH + n_authorities as u64);
        for c in certs.iter().filter(|c| c.round <= g) {
            prop_assert!(dag.is_tombstoned(&c.hash()));
        }
    }

    /// A certificate authored just above the gc round validates against
    /// its (now tombstoned) parents exactly as it would have before the
    /// prune; obsolete and orphan certificates get the documented errors.
    #[test]
    fn insert_verdicts_match_after_prune(
        n_authorities in 1u32..=7,
        n_rounds in 2u64..=10,
        omit_bits in any::<u64>(),
        parent_bits in any::<u64>(),
        g_frac in 0u64..=100,
    ) {
        let certs = random_dag(n_authorities, n_rounds, omit_bits, parent_bits);
        let orig = store_from(&certs);
        let max_round = orig.max_round().unwrap();
        let g = (max_round.saturating_sub(1)) * g_frac / 100;
        let mut pruned = orig.clone();
        pruned.prune_below(g);
        let mut orig = orig;

        // 1. New cert at g+1 naming every cert at round g as parent.
        let parents: Vec<CertHash> = orig.round_hashes(g).to_vec();
        let fresh = Certificate {
            author: n_authorities, // a new author id: never collides
            round: g + 1,
            parents: parents.clone(),
            payload_digest: [0xA5; 32],
            signature: Vec::new(),
        };
        let v_orig = orig.insert(fresh.clone());
        let v_pruned = pruned.insert(fresh);
        prop_assert!(v_pruned.is_ok(), "fresh cert above gc round rejected: {:?}", v_pruned);
        prop_assert_eq!(v_orig, v_pruned, "fresh cert above gc round: verdicts differ");

        // 2. An obsolete cert (round <= g) is BelowGcRound on the pruned
        //    store, whatever the unpruned verdict would have been.
        let stale = Certificate {
            author: n_authorities + 1,
            round: g,
            parents: if g == 0 { Vec::new() } else { orig.round_hashes(g - 1).to_vec() },
            payload_digest: [0x5A; 32],
            signature: Vec::new(),
        };
        prop_assert_eq!(
            pruned.insert(stale),
            Err(ConsensusError::BelowGcRound { round: g, gc_round: g })
        );

        // 3. A genuinely unknown parent is still an orphan.
        let orphan = Certificate {
            author: n_authorities + 2,
            round: g + 2,
            parents: vec![CertHash([0xEE; 32])],
            payload_digest: [0xEE; 32],
            signature: Vec::new(),
        };
        prop_assert!(matches!(pruned.insert(orphan), Err(ConsensusError::UnknownParent(_))));
    }

    /// Pruning removes exactly a prefix of the linearization.
    #[test]
    fn linearize_is_prefix_stable(
        n_authorities in 1u32..=7,
        n_rounds in 2u64..=12,
        omit_bits in any::<u64>(),
        parent_bits in any::<u64>(),
        g_frac in 0u64..=100,
    ) {
        let certs = random_dag(n_authorities, n_rounds, omit_bits, parent_bits);
        let orig = store_from(&certs);
        let max_round = orig.max_round().unwrap();
        let g = (max_round.saturating_sub(1)) * g_frac / 100;
        let mut pruned = orig.clone();
        pruned.prune_below(g);

        let expected: Vec<CertHash> = orig
            .linearize()
            .into_iter()
            .filter(|h| round_of(&orig, h) > g)
            .collect();
        prop_assert_eq!(pruned.linearize(), expected);
    }
}
