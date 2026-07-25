//! IQ-002 exit-gate property tests.
//!
//! Two complementary exit-gate properties cover both branches of the
//! indirect decision rule:
//!
//! - `indirect_resolves_to_direct_when_leader_threaded` — when a leader
//!   slot at round `R` fails the direct rule (insufficient supporters at
//!   `R+1`) but at least one `R+1` cert keeps the leader in its parent
//!   set (so the leader threads through to a dense `R+2`), the leader
//!   *is* in the causal history of a later directly-decided anchor and
//!   `decide_slot` must return `Direct(leader_R)`.
//! - `indirect_resolves_to_skip_when_leader_orphaned` — when *no* `R+1`
//!   cert references `R`'s leader (the leader is fully orphaned),
//!   `decide_slot` must return `Skip` once any later directly-decided
//!   anchor exists.
//!
//! Supporting properties:
//!
//! - `indirect_resolution_is_pure` — repeated calls to `decide_slot`
//!   on the same DAG return the same `LeaderStatus`.
//! - `indirect_resolution_is_monotone_under_extension` — extending the
//!   DAG with later rounds can flip `Undecided -> Direct/Skip`, but
//!   cannot flip `Direct -> Undecided/Skip` or `Skip -> Direct/Undecided`.
//!
//! Run at default 256 cases under CI; sprint close runs
//! `PROPTEST_CASES=10000 cargo test -p suwappu-consensus --release`.
//!
//! Tracks `docs/iq/IQ-002-indirect-commit.md`.

use proptest::prelude::*;
use suwappu_consensus::{
    cert_at, decide_slot, leader, quorum_threshold, try_direct_decide, AuthorityId, CertHash,
    Certificate, CommitteeSize, DagStore, LeaderStatus, Round,
};

/// Build a valid topo-ordered DAG of `n_rounds` rounds with the
/// following sparsity profile at the chosen `sparse_round`:
///
/// At round `sparse_round`, only `n_supporters` certs reference *all*
/// of round `sparse_round - 1`'s certs as parents; the remaining
/// `n_authorities - n_supporters` certs at `sparse_round` reference
/// only `quorum_threshold(n) - 1` of round `sparse_round - 1`'s certs
/// (omitting the leader). All other rounds are fully dense.
///
/// Effect: round `sparse_round - 1`'s leader has fewer than
/// `quorum_threshold(n)` direct supporters at `sparse_round`, forcing
/// the direct rule to return `Undecided`. The indirect rule must
/// resolve it from a later anchor.
fn build_sparse_dag(
    n_rounds: u64,
    n_authorities: CommitteeSize,
    sparse_round: u64,
    n_supporters: u32,
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

            let parents: Vec<CertHash> = if r == 0 {
                Vec::new()
            } else if r == sparse_round && a >= n_supporters {
                // Non-supporter at the sparse round: drop the
                // (r-1)-leader's cert from parents. Reference the
                // remaining (n-1) parents.
                let leader_a = leader(r - 1, n_authorities);
                prev_round_hashes
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, h)| {
                        if idx as AuthorityId == leader_a {
                            None
                        } else {
                            Some(*h)
                        }
                    })
                    .collect()
            } else {
                prev_round_hashes.clone()
            };

            let cert = if r == 0 {
                Certificate::genesis(a as AuthorityId, payload)
            } else {
                Certificate {
                    author: a as AuthorityId,
                    round: r as Round,
                    parents,
                    payload_digest: payload,
                    signature: Vec::new(),
                }
            };
            this_round.push(cert.hash());
            all.push(cert);
        }
        prev_round_hashes = this_round;
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

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 32,
        .. ProptestConfig::default()
    })]

    /// EXIT GATE A — a round whose direct rule returns `Undecided` but
    /// whose leader IS in the causal history of a later directly-decided
    /// anchor must be resolved to `Direct(leader_hash)` by `decide_slot`.
    /// "In causal history" is ensured here by keeping at least one R+1
    /// cert that still references the R-leader as parent.
    #[test]
    fn indirect_resolves_to_direct_when_leader_threaded(
        n_authorities in 4u32..=7,
        sparse_round in 1u64..=3,
        extra_anchor_rounds in 2u64..=4,
        // n_supporters >= 1 so the leader threads into the dense (R+2)
        // anchor's causal history via the surviving R+1 supporter(s);
        // < quorum so the direct rule still fails.
        support_offset in 0u32..=1,
        payload_seed in any::<u64>(),
    ) {
        let n = n_authorities;
        let quorum = quorum_threshold(n);
        // Map support_offset (0..=1) to n_supporters in
        // {1, max(1, quorum-1)}. For n=4, quorum=3, so n_supporters in
        // {1, 2}, both < 3 = quorum. For n=7, quorum=5, n_supporters in
        // {1, 4}. Always >= 1 (leader threaded), always < quorum
        // (direct rule fails).
        let n_supporters = 1u32.max(quorum.saturating_sub(1).min(1 + support_offset));
        prop_assume!(n_supporters >= 1 && n_supporters < quorum);

        let n_rounds = sparse_round + 1 + extra_anchor_rounds;
        let target_round = sparse_round - 1;

        let certs = build_sparse_dag(
            n_rounds,
            n,
            sparse_round,
            n_supporters,
            payload_seed,
        );
        let store = store_from(&certs);

        // Sanity: target leader cert exists.
        let target_leader_hash = cert_at(&store, target_round, leader(target_round, n))
            .expect("target round leader cert must exist in dense base");

        // Direct rule MUST be Undecided.
        prop_assert_eq!(
            try_direct_decide(&store, target_round, n),
            LeaderStatus::Undecided,
            "construction error: direct rule should be Undecided at target round {}",
            target_round,
        );

        // Sanity: there is a directly-decided anchor at >= target+2.
        let mut anchor: Option<Round> = None;
        for r in (target_round + 2)..n_rounds {
            if let LeaderStatus::Direct(_) = try_direct_decide(&store, r, n) {
                anchor = Some(r);
                break;
            }
        }
        prop_assert!(
            anchor.is_some(),
            "construction error: no directly-decided anchor at >= target+2 \
             (n={}, target={}, n_rounds={})",
            n, target_round, n_rounds,
        );

        match decide_slot(&store, target_round, n) {
            LeaderStatus::Direct(h) => {
                prop_assert_eq!(
                    h,
                    target_leader_hash,
                    "decide_slot returned wrong cert hash at target round {}",
                    target_round,
                );
            }
            other => {
                prop_assert!(
                    false,
                    "decide_slot returned {:?} for target round {}; expected Direct(_). \
                     n={}, sparse_round={}, n_supporters={}, n_rounds={}",
                    other, target_round, n, sparse_round, n_supporters, n_rounds,
                );
            }
        }
    }

    /// EXIT GATE B — a round whose leader is fully orphaned at R+1
    /// (zero R+1 certs reference the leader as parent) must resolve to
    /// `Skip` once any later directly-decided anchor exists. The leader
    /// cert exists in the DAG but is unreachable from the anchor's
    /// causal history, which is the structural condition for `Skip` per
    /// `try_indirect_decide`.
    #[test]
    fn indirect_resolves_to_skip_when_leader_orphaned(
        n_authorities in 4u32..=7,
        sparse_round in 1u64..=3,
        extra_anchor_rounds in 2u64..=4,
        payload_seed in any::<u64>(),
    ) {
        let n = n_authorities;
        let n_supporters = 0u32;
        let n_rounds = sparse_round + 1 + extra_anchor_rounds;
        let target_round = sparse_round - 1;

        let certs = build_sparse_dag(
            n_rounds,
            n,
            sparse_round,
            n_supporters,
            payload_seed,
        );
        let store = store_from(&certs);

        // Direct rule MUST be Undecided.
        prop_assert_eq!(
            try_direct_decide(&store, target_round, n),
            LeaderStatus::Undecided,
            "construction error: direct rule should be Undecided at target round {}",
            target_round,
        );

        // Sanity: anchor exists.
        let mut anchor: Option<Round> = None;
        for r in (target_round + 2)..n_rounds {
            if let LeaderStatus::Direct(_) = try_direct_decide(&store, r, n) {
                anchor = Some(r);
                break;
            }
        }
        prop_assert!(
            anchor.is_some(),
            "construction error: no directly-decided anchor at >= target+2 \
             (n={}, target={}, n_rounds={})",
            n, target_round, n_rounds,
        );

        prop_assert_eq!(
            decide_slot(&store, target_round, n),
            LeaderStatus::Skip,
            "decide_slot did not Skip an orphaned leader at target round {} \
             (n={}, sparse_round={}, n_rounds={})",
            target_round, n, sparse_round, n_rounds,
        );
    }

    /// Repeated calls to `decide_slot` on the same DAG return the same
    /// `LeaderStatus`. (Purity / determinism check.)
    #[test]
    fn indirect_resolution_is_pure(
        n_authorities in 4u32..=7,
        n_rounds in 3u64..=6,
        sparse_round in 1u64..=3,
        sparse_support_fraction in 0u32..=2,
        payload_seed in any::<u64>(),
    ) {
        let n = n_authorities;
        let quorum = quorum_threshold(n);
        let n_supporters = sparse_support_fraction.min(quorum.saturating_sub(1));
        let effective_sparse_round = sparse_round.min(n_rounds.saturating_sub(1).max(1));

        let certs = build_sparse_dag(
            n_rounds,
            n,
            effective_sparse_round,
            n_supporters,
            payload_seed,
        );
        let store = store_from(&certs);

        for r in 0..n_rounds {
            let a = decide_slot(&store, r, n);
            let b = decide_slot(&store, r, n);
            prop_assert_eq!(
                a, b,
                "decide_slot non-deterministic at round {}: {:?} != {:?}",
                r, a, b,
            );
        }
    }

    /// Extending the DAG by more dense rounds preserves any `Direct`
    /// decision and any `Skip` decision: it cannot reopen them. Only
    /// `Undecided` is allowed to flip — and only to `Direct` or `Skip`.
    #[test]
    fn indirect_resolution_is_monotone_under_extension(
        n_authorities in 4u32..=7,
        n_rounds in 3u64..=5,
        sparse_round in 1u64..=2,
        sparse_support_fraction in 0u32..=2,
        extra_rounds in 0u64..=4,
        payload_seed in any::<u64>(),
    ) {
        let n = n_authorities;
        let quorum = quorum_threshold(n);
        let n_supporters = sparse_support_fraction.min(quorum.saturating_sub(1));
        let effective_sparse_round = sparse_round.min(n_rounds.saturating_sub(1).max(1));

        let base = build_sparse_dag(
            n_rounds,
            n,
            effective_sparse_round,
            n_supporters,
            payload_seed,
        );
        let store_base = store_from(&base);

        let total_rounds = n_rounds + extra_rounds;
        let ext = build_sparse_dag(
            total_rounds,
            n,
            effective_sparse_round,
            n_supporters,
            payload_seed,
        );
        let store_ext = store_from(&ext);

        for r in 0..n_rounds {
            let base_status = decide_slot(&store_base, r, n);
            let ext_status = decide_slot(&store_ext, r, n);

            match base_status {
                LeaderStatus::Direct(h) => {
                    prop_assert_eq!(
                        ext_status,
                        LeaderStatus::Direct(h),
                        "extension demoted Direct -> {:?} at round {}",
                        ext_status, r,
                    );
                }
                LeaderStatus::Skip => {
                    prop_assert_eq!(
                        ext_status,
                        LeaderStatus::Skip,
                        "extension flipped Skip -> {:?} at round {}",
                        ext_status, r,
                    );
                }
                LeaderStatus::Undecided => {
                    // Either resolved or still undecided; both allowed.
                }
            }
        }
    }
}
