//! DAG-S14 exit-gate property tests.
//!
//! Exit gate: `reserve_coverage_predicate` — for any `(reserves,
//! outstanding, rule)` triple, the predicate's verdict matches the
//! arithmetic relation specified in paper §8.3.
//!
//! Supporting properties:
//!
//! - `failed_attestation_pauses_minting` — a failing attestation is
//!   rejected at submit; the breaker reports `NoFreshAttestation` until
//!   a passing one arrives.
//! - `fresh_passing_attestation_unpauses` — submitting a passing
//!   attestation enables `can_mint`.
//! - `stale_attestation_pauses` — `can_mint` returns
//!   `NoFreshAttestation` once `now_round` exceeds
//!   `attested_at + ttl_rounds`.
//!
//! Run at default 256 cases under CI; sprint close runs
//! `PROPTEST_CASES=10000 cargo test -p suwappu-precompiles --release`.

use proptest::prelude::*;
use suwappu_precompiles::{
    predicate_satisfied, AssetId, CoverageError, CoverageRule, DisclosureTier, ReserveAttestation,
    ReserveCoverageChecker,
};

fn rule_strategy() -> impl Strategy<Value = CoverageRule> {
    prop_oneof![
        Just(CoverageRule::OneToOnePar),
        (1u32..=20_000).prop_map(|bps| CoverageRule::NavStrike { basis_points: bps }),
        (1u32..=20_000).prop_map(|bps| CoverageRule::Jurisdiction { ratio_bps: bps }),
    ]
}

fn attestation(
    asset_seed: u8,
    reserves: u128,
    outstanding: u128,
    round: u64,
) -> ReserveAttestation {
    ReserveAttestation {
        issuer: 0,
        asset: AssetId([asset_seed; 32]),
        total_reserves: reserves,
        outstanding_at_attestation: outstanding,
        attested_at: round,
        schema_version: 1,
        tier: DisclosureTier::F1Aggregate,
        commitment: [0xAB; 32],
        proof: Vec::new(),
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 32,
        .. ProptestConfig::default()
    })]

    /// EXIT GATE — predicate verdict matches the arithmetic relation.
    ///
    /// Reserves and outstanding are bounded to avoid u128-overflow on
    /// the basis-point multiplication; the overflow path is exercised
    /// by the unit tests, and the predicate's response on overflow is
    /// documented as `CoverageError::Overflow`.
    #[test]
    fn reserve_coverage_predicate(
        rule in rule_strategy(),
        reserves in 0u128..=1_000_000_000,
        outstanding in 0u128..=1_000_000_000,
    ) {
        let actual = predicate_satisfied(rule, reserves, outstanding);
        let expected_pass = match rule {
            CoverageRule::OneToOnePar => reserves >= outstanding,
            CoverageRule::NavStrike { basis_points } => {
                reserves * 10_000 >= outstanding * basis_points as u128
            }
            CoverageRule::Jurisdiction { ratio_bps } => {
                reserves * 10_000 >= outstanding * ratio_bps as u128
            }
        };
        if expected_pass {
            prop_assert!(actual.is_ok());
        } else {
            let is_predicate_fail = matches!(
                actual,
                Err(CoverageError::PredicateFailed { .. })
            );
            prop_assert!(is_predicate_fail);
        }
    }

    /// A failing attestation is rejected at submit; the breaker stays
    /// in `NoFreshAttestation` until a passing attestation arrives.
    #[test]
    fn failed_attestation_pauses_minting(
        // Construct (reserves, outstanding) so reserves < outstanding
        // (the 1:1-par failure condition).
        (reserves, outstanding) in (1u128..=1_000_000)
            .prop_flat_map(|out| (0u128..out, Just(out))),
        asset_seed in any::<u8>(),
    ) {
        let asset = AssetId([asset_seed; 32]);
        let mut c = ReserveCoverageChecker::with_ttl(1_000);
        c.set_rule(0, asset, CoverageRule::OneToOnePar);
        let bad = attestation(asset_seed, reserves, outstanding, 10);
        let submit_err = c.submit_attestation(bad);
        let is_predicate_fail = matches!(
            submit_err,
            Err(CoverageError::PredicateFailed { .. })
        );
        prop_assert!(is_predicate_fail);

        // The breaker reports no fresh attestation until one passes.
        let mint_err = c.can_mint(0, asset, outstanding, 50);
        prop_assert_eq!(mint_err, Err(CoverageError::NoFreshAttestation));
    }

    /// A passing attestation submitted within TTL unpauses minting for
    /// outstanding at-or-below the attested level.
    #[test]
    fn fresh_passing_attestation_unpauses(
        // reserves ≥ outstanding for OneToOnePar pass.
        (outstanding, reserves) in (1u128..=1_000_000)
            .prop_flat_map(|out| (Just(out), out..=1_000_000_000)),
        asset_seed in any::<u8>(),
        attested_at in 0u64..=10_000,
        check_offset in 0u64..=500,
    ) {
        let asset = AssetId([asset_seed; 32]);
        let mut c = ReserveCoverageChecker::with_ttl(1_000);
        c.set_rule(0, asset, CoverageRule::OneToOnePar);
        c.submit_attestation(attestation(asset_seed, reserves, outstanding, attested_at))
            .unwrap();
        let check_at = attested_at + check_offset; // within TTL
        c.can_mint(0, asset, outstanding, check_at).unwrap();
    }

    /// `can_mint` returns `NoFreshAttestation` once the attestation has
    /// aged past `ttl_rounds`.
    #[test]
    fn stale_attestation_pauses(
        (outstanding, reserves) in (1u128..=1_000_000)
            .prop_flat_map(|out| (Just(out), out..=1_000_000_000)),
        asset_seed in any::<u8>(),
        attested_at in 0u64..=10_000,
        ttl in 1u64..=500,
        beyond in 1u64..=500,
    ) {
        let asset = AssetId([asset_seed; 32]);
        let mut c = ReserveCoverageChecker::with_ttl(ttl);
        c.set_rule(0, asset, CoverageRule::OneToOnePar);
        c.submit_attestation(attestation(asset_seed, reserves, outstanding, attested_at))
            .unwrap();
        let now = attested_at + ttl + beyond;
        let err = c.can_mint(0, asset, outstanding, now);
        prop_assert_eq!(err, Err(CoverageError::NoFreshAttestation));
    }
}
