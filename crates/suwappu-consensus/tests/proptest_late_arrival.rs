//! IQ-004 exit-gate property tests — late-arrival re-decide.
//!
//! When orphan-pull recovery delivers a cert *after* its round's first
//! directly-decided anchor has already been processed, the previously-
//! `Skip` slot must be allowed to resolve `Direct` from a *later*
//! directly-decided anchor whose updated `causal_history` reaches it.
//!
//! Test scenarios:
//!
//! - `decide_slot_scans_past_first_skip_anchor` — construct a DAG where
//!   the first directly-decided anchor's causal_history does NOT reach
//!   the target leader, but a later directly-decided anchor's does.
//!   Old `decide_slot` returned `Skip` (early-return on first anchor).
//!   New `decide_slot` (IQ-004) iterates and returns `Direct`.
//! - `late_arrival_does_not_break_direct_monotonicity` — extending a
//!   DAG that already returns `Direct(h)` for some round R must keep
//!   returning `Direct(h)` regardless of further certs inserted. (The
//!   monotonicity guarantee that `mysticeti_c_finality` already covers
//!   for the direct rule, restated for the new path.)
//! - `orphan_with_zero_supporters_still_skips` — when no R+1 cert
//!   references the leader, no anchor's causal_history can reach the
//!   leader; `decide_slot` must still return `Skip` after exhausting
//!   the search space. This is the unchanged guarantee from IQ-002
//!   exit gate B.
//!
//! Tracks `docs/iq/IQ-004-decide-slot-orphan-window.md`.

use proptest::prelude::*;
use suwappu_consensus::{
    cert_at, decide_slot, leader, try_direct_decide, AuthorityId, CertHash, Certificate,
    CommitteeSize, DagStore, LeaderStatus, Round,
};

/// Build a `Certificate` with the given author, round, parents, and a
/// payload digest derived from `seed`.
fn cert(author: AuthorityId, round: Round, parents: Vec<CertHash>, seed: u64) -> Certificate {
    let mut payload = [0u8; 32];
    payload[..8].copy_from_slice(&seed.to_le_bytes());
    payload[8] = author as u8;
    payload[9] = round as u8;
    if round == 0 {
        Certificate::genesis(author, payload)
    } else {
        Certificate {
            author,
            round,
            parents,
            payload_digest: payload,
        }
    }
}

fn store_with(certs: &[Certificate]) -> DagStore {
    let mut s = DagStore::new();
    for c in certs {
        s.insert(c.clone())
            .expect("topo-ordered insert must succeed");
    }
    s
}

/// Hand-built scenario covering the IQ-004 fix: the first directly-decided
/// anchor's causal_history misses the target leader, but a later one
/// reaches it via certs that thread the leader through a sparse R+1.
///
/// Topology (n=4, target_round=0, leader@0 = v0):
///
/// - R0: 4 genesis certs.
/// - R1: only v0@1 references v0@0 as parent; v1@1, v2@1, v3@1 omit it.
///   → direct rule on R0 is Undecided (1 supporter < quorum=3).
/// - R2: v0@2 references v0@1 (so its causal_history reaches leader);
///   v1@2, v2@2, v3@2 omit v0@1 (so their causal_history misses leader).
///   leader@2 = v2; v2@2's causal_history does NOT reach v0@0.
/// - R3: all 4 authors reference all R+2 certs → v2@2 (leader@2) is
///   directly decided by ≥ quorum=3 R+3 supporters. v2@2 is therefore
///   the FIRST directly-decided anchor at round ≥ target+2.
/// - R4: all 4 authors reference all R+3 certs → v3@3 (leader@3, since
///   3 mod 4 = 3) is directly decided. v3@3's causal_history includes
///   v0@2 (one of its R+2 parents); v0@2's causal_history includes
///   v0@1; v0@1's parents include v0@0 = leader. So
///   causal_history(v3@3) ⊇ {v0@0}.
///
/// Old `decide_slot` returned Skip after failing on v2@2.
/// New `decide_slot` (IQ-004) scans past v2@2 to v3@3 and returns Direct.
#[test]
fn decide_slot_scans_past_first_skip_anchor() {
    let n: CommitteeSize = 4;
    let target_round: Round = 0;
    let target_author = leader(target_round, n);
    assert_eq!(target_author, 0);

    let mut all: Vec<Certificate> = Vec::new();

    // R0
    let r0: Vec<Certificate> = (0..n)
        .map(|a| cert(a as AuthorityId, 0, Vec::new(), 0))
        .collect();
    let r0h: Vec<CertHash> = r0.iter().map(|c| c.hash()).collect();
    let leader_hash = r0h[target_author as usize];
    all.extend(r0);

    // R1: only v0@1 keeps v0@0 as parent.
    let r1: Vec<Certificate> = (0..n)
        .map(|a| {
            let parents = if a == 0 {
                r0h.clone()
            } else {
                r0h.iter()
                    .enumerate()
                    .filter_map(|(idx, h)| {
                        if idx as AuthorityId == target_author {
                            None
                        } else {
                            Some(*h)
                        }
                    })
                    .collect()
            };
            cert(a as AuthorityId, 1, parents, 1)
        })
        .collect();
    let r1h: Vec<CertHash> = r1.iter().map(|c| c.hash()).collect();
    all.extend(r1);

    // R2: only v0@2 keeps v0@1; v1@2/v2@2/v3@2 drop v0@1 from parents.
    // This makes v2@2 (leader@2) directly decidable at R+3 but with
    // causal_history that excludes v0@0.
    let r2: Vec<Certificate> = (0..n)
        .map(|a| {
            let parents = if a == 0 {
                r1h.clone()
            } else {
                r1h.iter()
                    .enumerate()
                    .filter_map(|(idx, h)| if idx == 0 { None } else { Some(*h) })
                    .collect()
            };
            cert(a as AuthorityId, 2, parents, 2)
        })
        .collect();
    let r2h: Vec<CertHash> = r2.iter().map(|c| c.hash()).collect();
    all.extend(r2);

    // R3: all authors reference all R+2 certs → v2@2 directly decided.
    let r3: Vec<Certificate> = (0..n)
        .map(|a| cert(a as AuthorityId, 3, r2h.clone(), 3))
        .collect();
    let r3h: Vec<CertHash> = r3.iter().map(|c| c.hash()).collect();
    all.extend(r3);

    // R4: all authors reference all R+3 certs → v3@3 directly decided.
    let r4: Vec<Certificate> = (0..n)
        .map(|a| cert(a as AuthorityId, 4, r3h.clone(), 4))
        .collect();
    all.extend(r4);

    let store = store_with(&all);

    // Sanity: direct rule at target_round=0 is Undecided.
    assert_eq!(
        try_direct_decide(&store, 0, n),
        LeaderStatus::Undecided,
        "construction error: direct decide should be Undecided at target round 0",
    );

    // Sanity: v2@2 (leader@2) is directly decided AND its
    // causal_history excludes leader@0 (so old decide_slot returns
    // Skip after evaluating this anchor).
    let v2_at_2 = cert_at(&store, 2, leader(2, n)).expect("v2@2 must exist");
    assert!(matches!(
        try_direct_decide(&store, 2, n),
        LeaderStatus::Direct(h) if h == v2_at_2,
    ));

    // Sanity: v3@3 (leader@3) is directly decided AND its
    // causal_history DOES include leader@0.
    assert_eq!(leader(3, n), 3);
    let v3_at_3 = cert_at(&store, 3, 3).expect("v3@3 must exist");
    assert!(matches!(
        try_direct_decide(&store, 3, n),
        LeaderStatus::Direct(h) if h == v3_at_3,
    ));

    // KEY ASSERTION: decide_slot scans past v2@2's Skip and finds
    // v3@3 has leader@0 in its causal_history.
    assert_eq!(
        decide_slot(&store, 0, n),
        LeaderStatus::Direct(leader_hash),
        "IQ-004 fix: decide_slot must scan past the first directly-decided \
         anchor whose causal_history doesn't reach the target leader",
    );
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 32,
        .. ProptestConfig::default()
    })]

    /// Direct-decided slots stay Direct under DAG extension. This is the
    /// monotonicity guarantee the new path must preserve.
    #[test]
    fn late_arrival_does_not_break_direct_monotonicity(
        n_authorities in 4u32..=7,
        extra_rounds in 1u64..=5,
        payload_seed in any::<u64>(),
    ) {
        let n = n_authorities;
        // Build a dense DAG where every round directly commits, then
        // extend with `extra_rounds` more dense rounds and verify the
        // initial commits are unchanged.
        let initial_rounds: u64 = 4;

        // Helper: build a dense DAG of `n_rounds` rounds.
        let build = |n_rounds: u64, seed_offset: u64| -> Vec<Certificate> {
            let mut all = Vec::new();
            let mut prev: Vec<CertHash> = Vec::new();
            for r in 0..n_rounds {
                let mut this: Vec<CertHash> = Vec::with_capacity(n as usize);
                for a in 0..n {
                    let parents = if r == 0 { Vec::new() } else { prev.clone() };
                    let c = cert(a as AuthorityId, r as Round, parents, payload_seed.wrapping_add(seed_offset).wrapping_add(r));
                    this.push(c.hash());
                    all.push(c);
                }
                prev = this;
            }
            all
        };

        let initial = build(initial_rounds, 0);
        let store_initial = store_with(&initial);
        let mut initial_decisions: Vec<(Round, LeaderStatus)> = Vec::new();
        for r in 0..initial_rounds {
            initial_decisions.push((r, decide_slot(&store_initial, r as Round, n)));
        }

        let extended = build(initial_rounds + extra_rounds, 0);
        let store_extended = store_with(&extended);

        for (r, prev_status) in &initial_decisions {
            if let LeaderStatus::Direct(h) = prev_status {
                let ext_status = decide_slot(&store_extended, *r, n);
                prop_assert_eq!(
                    ext_status,
                    LeaderStatus::Direct(*h),
                    "Direct({:?}) at round {} must stay Direct under extension; got {:?}",
                    h, r, ext_status,
                );
            }
        }
    }

    /// When zero R+1 certs reference the target leader, no anchor's
    /// causal_history can reach it. The IQ-004 fix is monotone:
    /// `Skip` here must remain `Skip` (we only added a scan, we did
    /// not add a path that wasn't already in the DAG).
    ///
    /// This mirrors `indirect_resolves_to_skip_when_leader_orphaned`
    /// from `proptest_indirect_commit` but verified end-to-end through
    /// the new multi-anchor scan.
    #[test]
    fn orphan_with_zero_supporters_still_skips(
        n_authorities in 4u32..=7,
        extra_anchor_rounds in 2u64..=5,
        payload_seed in any::<u64>(),
    ) {
        let n = n_authorities;
        let target_round: Round = 0;
        let target_author = leader(target_round, n);

        // R0: 4 genesis certs.
        let r0: Vec<Certificate> = (0..n)
            .map(|a| cert(a as AuthorityId, 0, Vec::new(), payload_seed))
            .collect();
        let r0h: Vec<CertHash> = r0.iter().map(|c| c.hash()).collect();
        let mut all = r0;

        // R1: ZERO authors reference leader@0. All R+1 certs use a
        // parent set that excludes leader. Direct rule on R0 is
        // Undecided (0 supporters < quorum).
        let r1: Vec<Certificate> = (0..n)
            .map(|a| {
                let parents = r0h
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, h)| {
                        if idx as AuthorityId == target_author {
                            None
                        } else {
                            Some(*h)
                        }
                    })
                    .collect();
                cert(a as AuthorityId, 1, parents, payload_seed.wrapping_add(1))
            })
            .collect();
        let r1h: Vec<CertHash> = r1.iter().map(|c| c.hash()).collect();
        all.extend(r1);

        // Dense rounds R2..R2+extra_anchor_rounds with all-parents-included
        // so anchors are directly decided.
        let mut prev = r1h;
        for r in 2u64..(2 + extra_anchor_rounds) {
            let this: Vec<Certificate> = (0..n)
                .map(|a| cert(a as AuthorityId, r as Round, prev.clone(), payload_seed.wrapping_add(r)))
                .collect();
            let this_h: Vec<CertHash> = this.iter().map(|c| c.hash()).collect();
            all.extend(this);
            prev = this_h;
        }

        let store = store_with(&all);

        prop_assert_eq!(
            try_direct_decide(&store, target_round, n),
            LeaderStatus::Undecided,
            "construction: direct rule should be Undecided at target round 0",
        );

        // No R+1 cert lists leader as parent; no anchor's causal_history
        // can reach it. The new multi-anchor scan must exhaust without
        // finding leader → Skip.
        prop_assert_eq!(
            decide_slot(&store, target_round, n),
            LeaderStatus::Skip,
            "orphaned-leader scenario must still resolve to Skip after \
             IQ-004 multi-anchor scan",
        );
    }
}
