//! IQ-002 exit-gate property test.
//!
//! Property: `indirect_commit_resolves_undecided_slots` — if a leader slot
//! at round `R` fails the direct decision rule (fewer than
//! `quorum_threshold(n)` supporters at round `R+1`) but is in the causal
//! history of a later directly-decided anchor at round `R' >= R+2`, then
//! `decide_slot` resolves `R` to `Direct(leader_R)`, not `Undecided`.
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
//! `PROPTEST_CASES=10000 cargo test -p gsx-consensus --release`.
//!
//! Tracks `docs/iq/IQ-002-indirect-commit.md`.

use gsx_consensus::{
    cert_at, decide_slot, leader, quorum_threshold, try_direct_decide, AuthorityId, CertHash,
    Certificate, CommitteeSize, DagStore, LeaderStatus, Round,
};
use proptest::prelude::*;

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

    /// EXIT GATE — a round whose direct rule returns `Undecided` but
    /// whose leader is in the causal history of a later directly-decided
    /// anchor must be resolved to `Direct` by `decide_slot`.
    #[test]
    fn indirect_commit_resolves_undecided_slots(
        n_authorities in 4u32..=7,
        // target = sparse_round - 1. We need an anchor at >= target+2,
        // so we need n_rounds >= target+3 = sparse_round+2. Pick
        // n_rounds in [sparse_round+2, sparse_round+5].
        sparse_round in 1u64..=3,
        extra_anchor_rounds in 2u64..=4,
        sparse_support_fraction in 0u32..=2,
        payload_seed in any::<u64>(),
    ) {
        let n = n_authorities;
        let quorum = quorum_threshold(n);
        // Number of certs at sparse_round that DO support
        // sparse_round - 1's leader: strictly fewer than quorum so
        // the direct rule fails. We map sparse_support_fraction
        // (0..=2) into [0, max(0, quorum-2)] inclusive.
        let n_supporters = if quorum == 0 {
            0
        } else {
            sparse_support_fraction.min(quorum.saturating_sub(1))
        };

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

        // Sanity: the target round's leader cert exists in the DAG.
        let target_leader_hash = cert_at(&store, target_round, leader(target_round, n))
            .expect("target round leader cert must exist in dense base");

        // Direct rule MUST be Undecided for the target round (n_supporters
        // < quorum is enforced above).
        prop_assert_eq!(
            try_direct_decide(&store, target_round, n),
            LeaderStatus::Undecided,
            "construction error: direct rule should be Undecided at target round {}",
            target_round,
        );

        // Find the lowest anchor round at >= target_round + 2 that is
        // directly decided. The dense rounds after `sparse_round`
        // (which is target_round + 1) should provide this.
        let mut anchor: Option<(Round, CertHash)> = None;
        for r in (target_round + 2)..n_rounds {
            if let LeaderStatus::Direct(h) = try_direct_decide(&store, r, n) {
                anchor = Some((r, h));
                break;
            }
        }
        prop_assert!(
            anchor.is_some(),
            "construction error: no directly-decided anchor at >= target+2 \
             (n={}, target={}, n_rounds={})",
            n, target_round, n_rounds,
        );

        // The top-level decide_slot must resolve target_round to Direct,
        // and the resolved hash must equal the target leader's cert hash
        // (causal history from the dense anchor reaches the target leader
        // via the n_supporters >= 0 dense path from sparse_round+1 onward).
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
