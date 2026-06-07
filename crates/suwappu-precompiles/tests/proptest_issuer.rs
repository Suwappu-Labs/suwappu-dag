//! DAG-S13 exit-gate property tests.
//!
//! Exit gate: `issuer_mint_burn_atomic` — for any mint followed by a
//! two-phase burn (initiate then finalize) of the same amount, the
//! issuer's outstanding and circulating supply both return to the
//! pre-mint state. The duplicate-redemption attack vector is closed by
//! the burn-id uniqueness invariant.
//!
//! Supporting properties:
//!
//! - `mint_respects_delegation_cap` — minting that would push
//!   outstanding supply above the issuer's `delegation_cap` fails.
//! - `duplicate_finalize_burn_rejected` — once a `BurnId` is finalized,
//!   a second `finalize_burn` on the same id returns `UnknownBurn`.
//! - `expired_burn_can_be_reversed` — `reverse_burn` succeeds strictly
//!   after the SLA deadline and fails at-or-before.
//!
//! Run at default 256 cases under CI; sprint close runs
//! `PROPTEST_CASES=10000 cargo test -p suwappu-precompiles --release`.

use suwappu_precompiles::{
    AssetId, Did, Issuer, IssuerError, IssuerRegistry, PaymentReceiptAttestation,
};
use proptest::prelude::*;

fn build_issuer(id: u32, cap: u128) -> Issuer {
    Issuer {
        id,
        principal_did: Did([id as u8; 32]),
        delegation_cap: cap,
        reserve_schema_version: 1,
        policy_vocabulary_version: 1,
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 32,
        .. ProptestConfig::default()
    })]

    /// EXIT GATE — mint(n) ∘ finalize_burn(n) returns the issuer's
    /// supply book-keeping to the pre-mint state.
    #[test]
    fn issuer_mint_burn_atomic(
        // Construct (cap, amount) so amount ∈ [1, cap]. Strategy
        // composition avoids the prop_assume reject loop at 10k cases.
        (cap, amount) in (1u128..=1_000_000)
            .prop_flat_map(|cap| (Just(cap), 1u128..=cap)),
        asset_seed in any::<u8>(),
        burn_round in 0u64..=10_000,
    ) {
        let asset = AssetId([asset_seed; 32]);
        let mut r = IssuerRegistry::with_sla(1_000);
        r.register(build_issuer(0, cap)).unwrap();

        let before = r.supply(asset, 0);
        r.mint(0, asset, amount).unwrap();
        let burn_id = r.initiate_burn(0, asset, amount, burn_round).unwrap();
        let att = PaymentReceiptAttestation { digest: [0xAB; 32] };
        r.finalize_burn(burn_id, att).unwrap();
        let after = r.supply(asset, 0);

        // Outstanding and circulating round-trip; total_minted and
        // total_burned both increased by `amount` but their difference
        // (the outstanding) is preserved.
        prop_assert_eq!(after.outstanding(), before.outstanding());
        prop_assert_eq!(after.circulating(), before.circulating());
        prop_assert_eq!(after.total_minted, before.total_minted + amount);
        prop_assert_eq!(after.total_burned, before.total_burned + amount);
        prop_assert_eq!(r.pending_burn_count(), 0);
    }

    /// Mints whose post-mint outstanding exceeds the delegation cap fail.
    #[test]
    fn mint_respects_delegation_cap(
        cap in 1u128..=1_000_000,
        first_amount in 1u128..=1_000_000,
        second_amount in 1u128..=1_000_000,
        asset_seed in any::<u8>(),
    ) {
        let asset = AssetId([asset_seed; 32]);
        let mut r = IssuerRegistry::new();
        r.register(build_issuer(0, cap)).unwrap();

        let first_result = r.mint(0, asset, first_amount);
        match first_result {
            Ok(()) => {
                prop_assert!(first_amount <= cap);
                let second_result = r.mint(0, asset, second_amount);
                let total_attempted = first_amount.saturating_add(second_amount);
                if total_attempted <= cap {
                    prop_assert!(second_result.is_ok());
                } else {
                    let is_cap_error = matches!(
                        second_result,
                        Err(IssuerError::DelegationCapExceeded { .. })
                    );
                    prop_assert!(is_cap_error);
                }
            }
            Err(IssuerError::DelegationCapExceeded { .. }) => {
                prop_assert!(first_amount > cap);
            }
            Err(e) => prop_assert!(false, "unexpected error: {:?}", e),
        }
    }

    /// Once a burn is finalized, a second finalize on the same id
    /// returns `UnknownBurn`. This is the structural invariant that
    /// closes the duplicate-redemption attack vector.
    #[test]
    fn duplicate_finalize_burn_rejected(
        cap in 100u128..=1_000_000,
        amount in 1u128..=100,
        asset_seed in any::<u8>(),
    ) {
        let asset = AssetId([asset_seed; 32]);
        let mut r = IssuerRegistry::with_sla(1_000);
        r.register(build_issuer(0, cap)).unwrap();
        r.mint(0, asset, amount).unwrap();
        let burn_id = r.initiate_burn(0, asset, amount, 0).unwrap();
        let att = PaymentReceiptAttestation { digest: [0; 32] };
        r.finalize_burn(burn_id, att).unwrap();
        let second = r.finalize_burn(burn_id, att);
        prop_assert_eq!(second, Err(IssuerError::UnknownBurn(burn_id)));
    }

    /// `reverse_burn` succeeds strictly after the SLA deadline; at or
    /// before, it fails with `SlaNotExpired`.
    #[test]
    fn expired_burn_can_be_reversed(
        cap in 100u128..=1_000_000,
        amount in 1u128..=100,
        sla_window in 1u64..=1_000,
        initiated_at in 0u64..=10_000,
        check_offset in 0i64..=2_000,
        asset_seed in any::<u8>(),
    ) {
        let asset = AssetId([asset_seed; 32]);
        let mut r = IssuerRegistry::with_sla(sla_window);
        r.register(build_issuer(0, cap)).unwrap();
        r.mint(0, asset, amount).unwrap();
        let burn_id = r.initiate_burn(0, asset, amount, initiated_at).unwrap();

        let deadline = initiated_at + sla_window;
        let check_at = ((deadline as i64) + check_offset).max(0) as u64;
        let result = r.reverse_burn(burn_id, check_at);
        if check_at > deadline {
            prop_assert!(result.is_ok());
            // After successful reversal, no pending burn remains.
            prop_assert_eq!(r.pending_burn_count(), 0);
        } else {
            let is_sla_error = matches!(result, Err(IssuerError::SlaNotExpired { .. }));
            prop_assert!(is_sla_error);
        }
    }
}
