//! Compute-incentive settlement exit-gate property tests.
//!
//! Exit gate: settlement_conservation_and_reserve_backing — for any
//! batch of compute receipts, pricing parameters, and reserve level, a
//! successful settlement mints exactly the sum of its payouts, never
//! exceeds the epoch budget, and leaves the post-mint outstanding
//! supply covered by the attested reserves.
//!
//! Supporting properties:
//! - coverage_failure_fails_closed — when reserves cannot cover the
//!   projected payout, nothing mints, supply is unchanged, and the
//!   epoch remains retryable.
//! - more_work_never_pays_less — a provider signing more certificates
//!   never receives a smaller payout, budget clamp included.
//! - epoch_replay_never_double_mints — settling the same epoch twice is
//!   rejected and mints nothing the second time.
//!
//! Run at default 256 cases under CI; sprint close runs
//! PROPTEST_CASES=10000 cargo test -p suwappu-precompiles --release

use proptest::prelude::*;

use suwappu_precompiles::did::Did;
use suwappu_precompiles::issuer::{AssetId, Issuer, IssuerId, IssuerRegistry};
use suwappu_precompiles::reserve::{
    predicate_satisfied, CoverageRule, DisclosureTier, ReserveAttestation, ReserveCoverageChecker,
};
use suwappu_precompiles::rewards::{ComputeReceipt, RewardError, RewardParams, RewardSettlement};

const ASSET: AssetId = AssetId([0xEE; 32]);
const ISSUER: IssuerId = 3;

fn build_state(reserves: u128) -> (IssuerRegistry, ReserveCoverageChecker) {
    let mut issuers = IssuerRegistry::new();
    issuers
        .register(Issuer {
            id: ISSUER,
            principal_did: Did([3; 32]),
            delegation_cap: u128::MAX,
            reserve_schema_version: 1,
            policy_vocabulary_version: 1,
        })
        .expect("register issuer");
    let mut coverage = ReserveCoverageChecker::with_ttl(1_000_000);
    coverage.set_rule(ISSUER, ASSET, CoverageRule::OneToOnePar);
    coverage
        .submit_attestation(ReserveAttestation {
            issuer: ISSUER,
            asset: ASSET,
            total_reserves: reserves,
            outstanding_at_attestation: 0,
            attested_at: 0,
            schema_version: 1,
            tier: DisclosureTier::F1Aggregate,
            commitment: [0; 32],
            proof: Vec::new(),
        })
        .expect("attestation with zero outstanding always passes");
    (issuers, coverage)
}

fn receipt_strategy(index: u8, epoch: u64) -> impl Strategy<Value = ComputeReceipt> {
    (
        1u64..10_000,  // certificates
        0u64..10_000,  // attestations
        0u64..1 << 34, // da bytes (up to 16 GiB)
        1u64..=1_000,  // total uptime samples
    )
        .prop_flat_map(move |(certs, attests, bytes, total)| {
            // Bias toward payable uptime (composed, not prop_assume'd —
            // a uniform 0..=total would reject most cases at 10k runs);
            // the low-uptime tail still gets exercised.
            let payable = (total * 95 / 100)..=total;
            let any = 0u64..=total;
            prop_oneof![4 => payable, 1 => any].prop_map(move |ok| ComputeReceipt {
                recipient: [index; 20],
                epoch,
                certificates_signed: certs,
                uptime_ok_samples: ok,
                uptime_total_samples: total,
                attestations_signed: attests,
                da_bytes_served: bytes,
            })
        })
}

fn receipts_strategy(epoch: u64) -> impl Strategy<Value = Vec<ComputeReceipt>> {
    // Distinct recipient per index — duplicates are a separate rejection
    // path, tested in unit tests.
    (1usize..=8).prop_flat_map(move |n| {
        (0..n as u8)
            .map(|i| receipt_strategy(i, epoch))
            .collect::<Vec<_>>()
    })
}

fn params_strategy() -> impl Strategy<Value = RewardParams> {
    (
        1u128..=1_000,
        1u128..=1_000,
        1u128..=1_000_000,
        1u128..=10_000_000,
    )
        .prop_map(
            |(per_certificate, per_attestation, per_gib_served, epoch_budget)| RewardParams {
                per_certificate,
                per_attestation,
                per_gib_served,
                epoch_budget,
            },
        )
}

proptest! {
    // No explicit `cases` override: proptest's default is 256, and
    // leaving it unset lets `PROPTEST_CASES=10000` (the sprint exit
    // gate) actually take effect — an inline `cases: 256` would
    // silently pin the count and ignore the environment variable.
    #![proptest_config(ProptestConfig {
        max_shrink_iters: 32,
        .. ProptestConfig::default()
    })]

    /// EXIT GATE — a successful settlement conserves value and stays
    /// reserve-backed: total_minted == Σ payouts <= epoch_budget, the
    /// issuer's outstanding supply grows by exactly total_minted, and
    /// the coverage predicate holds at the post-mint outstanding.
    #[test]
    fn settlement_conservation_and_reserve_backing(
        receipts in receipts_strategy(1),
        params in params_strategy(),
        reserves in 0u128..=100_000_000,
    ) {
        let (mut issuers, coverage) = build_state(reserves);
        let mut settlement = RewardSettlement::new(params.clone(), ISSUER, ASSET);
        let before = issuers.supply(ASSET, ISSUER).outstanding();

        match settlement.settle_epoch(1, &receipts, &mut issuers, &coverage, 10) {
            Ok(out) => {
                let payout_sum: u128 = out.payouts.iter().map(|(_, amount)| amount).sum();
                prop_assert_eq!(out.total_minted, payout_sum);
                prop_assert!(out.total_minted <= params.epoch_budget);
                let after = issuers.supply(ASSET, ISSUER).outstanding();
                prop_assert_eq!(after - before, out.total_minted);
                // Post-mint outstanding is still covered by reserves.
                if out.total_minted > 0 {
                    prop_assert!(
                        predicate_satisfied(CoverageRule::OneToOnePar, reserves, after).is_ok()
                    );
                }
                // Every listed payout is strictly positive.
                prop_assert!(out.payouts.iter().all(|(_, amount)| *amount > 0));
            }
            Err(RewardError::Coverage(_)) => {
                // Fail-closed is the other legal outcome; supply untouched.
                prop_assert_eq!(issuers.supply(ASSET, ISSUER).outstanding(), before);
            }
            Err(e) => return Err(TestCaseError::fail(format!("unexpected error: {e}"))),
        }
    }

    /// When reserves are strictly below the settled total, the breaker
    /// refuses the mint: supply is unchanged, the watermark does not
    /// advance, and the identical epoch settles once reserves cover it.
    #[test]
    fn coverage_failure_fails_closed(
        mut receipts in receipts_strategy(1),
        params in params_strategy(),
    ) {
        // Guarantee a strictly positive settled total by construction
        // (no prop_assume — a zero-total batch would blow the global
        // reject budget at high case counts): one full-uptime receipt
        // with at least one certificate always earns >= 1 unit.
        receipts.push(ComputeReceipt {
            recipient: [0xFF; 20],
            epoch: 1,
            certificates_signed: 1,
            uptime_ok_samples: 100,
            uptime_total_samples: 100,
            attestations_signed: 0,
            da_bytes_served: 0,
        });

        // Dry-run against unlimited reserves to learn the settled total.
        let (mut probe_issuers, probe_coverage) = build_state(u128::MAX);
        let mut probe = RewardSettlement::new(params.clone(), ISSUER, ASSET);
        let total = probe
            .settle_epoch(1, &receipts, &mut probe_issuers, &probe_coverage, 10)
            .expect("unlimited reserves always settle")
            .total_minted;
        if total == 0 {
            // Degenerate pro-rata clamp: a tiny budget spread across
            // many providers can floor every share to zero. Nothing to
            // gate — the zero-mint settlement above already succeeded.
            return Ok(());
        }

        // Now attempt with reserves one unit short of the total.
        let (mut issuers, coverage) = build_state(total - 1);
        let mut settlement = RewardSettlement::new(params, ISSUER, ASSET);
        let err = settlement.settle_epoch(1, &receipts, &mut issuers, &coverage, 10);
        prop_assert!(matches!(err, Err(RewardError::Coverage(_))));
        prop_assert_eq!(issuers.supply(ASSET, ISSUER).outstanding(), 0);
        prop_assert_eq!(settlement.last_settled_epoch(), None);

        // A fresh attestation covering the total unblocks the same epoch.
        let (mut issuers_ok, coverage_ok) = build_state(total);
        let out = settlement
            .settle_epoch(1, &receipts, &mut issuers_ok, &coverage_ok, 10)
            .expect("covered settlement succeeds");
        prop_assert_eq!(out.total_minted, total);
    }

    /// A provider who signs more certificates never receives a smaller
    /// payout, with everything else (other providers, budget) fixed.
    #[test]
    fn more_work_never_pays_less(
        base in receipts_strategy(1),
        params in params_strategy(),
        extra_certs in 1u64..1_000,
    ) {
        let (mut issuers_a, coverage_a) = build_state(u128::MAX);
        let mut settlement_a = RewardSettlement::new(params.clone(), ISSUER, ASSET);
        let out_a = settlement_a
            .settle_epoch(1, &base, &mut issuers_a, &coverage_a, 10)
            .expect("unlimited reserves always settle");

        let mut boosted = base.clone();
        boosted[0].certificates_signed += extra_certs;
        let (mut issuers_b, coverage_b) = build_state(u128::MAX);
        let mut settlement_b = RewardSettlement::new(params, ISSUER, ASSET);
        let out_b = settlement_b
            .settle_epoch(1, &boosted, &mut issuers_b, &coverage_b, 10)
            .expect("unlimited reserves always settle");

        let target = base[0].recipient;
        let paid = |s: &suwappu_precompiles::rewards::EpochSettlement| {
            s.payouts
                .iter()
                .find(|(r, _)| *r == target)
                .map(|(_, amount)| *amount)
                .unwrap_or(0)
        };
        prop_assert!(paid(&out_b) >= paid(&out_a));
    }

    /// Settling the same epoch twice is rejected and mints nothing the
    /// second time; a later epoch still settles.
    #[test]
    fn epoch_replay_never_double_mints(
        receipts in receipts_strategy(1),
        params in params_strategy(),
    ) {
        let (mut issuers, coverage) = build_state(u128::MAX);
        let mut settlement = RewardSettlement::new(params, ISSUER, ASSET);
        settlement
            .settle_epoch(1, &receipts, &mut issuers, &coverage, 10)
            .expect("unlimited reserves always settle");
        let after_first = issuers.supply(ASSET, ISSUER).outstanding();

        let err = settlement.settle_epoch(1, &receipts, &mut issuers, &coverage, 10);
        let replay_rejected = matches!(err, Err(RewardError::EpochNotMonotonic { .. }));
        prop_assert!(replay_rejected);
        prop_assert_eq!(issuers.supply(ASSET, ISSUER).outstanding(), after_first);

        // Receipts for a later epoch settle normally.
        let later: Vec<ComputeReceipt> = receipts
            .iter()
            .map(|r| ComputeReceipt { epoch: 2, ..r.clone() })
            .collect();
        settlement
            .settle_epoch(2, &later, &mut issuers, &coverage, 10)
            .expect("later epoch settles");
    }
}
