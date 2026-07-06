//! FEE-1 Phase 1 (IQ-007 Option A) fee-settlement property tests.
//!
//! Exercises `Substrate::apply_intent_with_fee` /
//! `execute_block_with_fees` on `InMemorySubstrate` ONLY. Full
//! mock-vs-prod parity (`InMemorySubstrate` vs `SuwappuDbSubstrate`)
//! genuinely needs the `suwappu-db` git dependency available and is
//! deferred to the settlement PR; the guards in `apply_intent_with_fee`
//! (zero-fee reject, reserved-payer reject) pin the cases the two impls
//! are guaranteed to agree on so that deferred parity proptest is sound.
//!
//! Properties:
//!
//! - `fee_less_path_matches_plain_execute_block` — an all-`None` fee
//!   slice yields a state root byte-identical to plain `execute_block`
//!   (the fee surface is a pure addition).
//! - `sponsored_charge_is_atomic_and_conserves_supply` — for a random
//!   `(intent, Option<FeeCharge>)`: on any failure (fee leg or intent
//!   leg) the state root is unchanged (atomicity); on success total
//!   supply is conserved (transfer + fee are internal balance moves,
//!   never a mint/burn).
//!
//! Run at default 256 cases under CI; sprint close runs
//! `PROPTEST_CASES=10000 cargo test -p suwappu-execution --release`.

use proptest::prelude::*;
use suwappu_execution::{
    execute_block, execute_block_with_fees, Address, Balance, Block, FeeCharge, InMemorySubstrate,
    Intent, Substrate,
};

/// Address keyed on a small u8 seed, so transfers/fees collide
/// realistically. Small seeds are never blake3-derived reserved
/// addresses, so a `payer = [seed; 20]` is a valid (non-reserved) payer.
fn addr_strategy() -> impl Strategy<Value = Address> {
    (0u8..5).prop_map(|seed| [seed; 20])
}

/// A single `Intent::Transfer`.
fn intent_strategy() -> impl Strategy<Value = Intent> {
    (addr_strategy(), addr_strategy(), 0u128..=1_000)
        .prop_map(|(from, to, amount)| Intent::Transfer { from, to, amount })
}

/// An optional fee. `max_fee` is always `> 0` (a zero fee is rejected by
/// `apply_intent_with_fee` and must be expressed as `None`); the payer is
/// a small-seed (non-reserved) address.
fn fee_strategy() -> impl Strategy<Value = Option<FeeCharge>> {
    prop_oneof![
        Just(None),
        (addr_strategy(), 1u128..=1_000)
            .prop_map(|(payer, max_fee)| Some(FeeCharge { payer, max_fee })),
    ]
}

/// Small initial-balance map (1–5 addresses).
fn initial_balances_strategy() -> impl Strategy<Value = Vec<(Address, Balance)>> {
    prop::collection::vec((addr_strategy(), 1u128..=10_000), 1..=5)
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 32,
        .. ProptestConfig::default()
    })]

    /// An all-`None` fee slice is byte-for-byte the fee-less path.
    #[test]
    fn fee_less_path_matches_plain_execute_block(
        initial in initial_balances_strategy(),
        intents in prop::collection::vec(intent_strategy(), 0..=8),
    ) {
        let mut s1 = InMemorySubstrate::from_balances(initial.clone());
        let mut s2 = InMemorySubstrate::from_balances(initial);
        let block = Block { round: 0, intents: intents.clone() };
        execute_block(&mut s1, &block);
        let fees: Vec<Option<FeeCharge>> = intents.iter().map(|_| None).collect();
        execute_block_with_fees(&mut s2, &block, &fees);
        prop_assert_eq!(s1.state_root(), s2.state_root());
    }

    /// Single `(intent, fee)`: failures are atomic; successes conserve
    /// total supply (fee + transfer are internal moves).
    #[test]
    fn sponsored_charge_is_atomic_and_conserves_supply(
        initial in initial_balances_strategy(),
        intent in intent_strategy(),
        fee in fee_strategy(),
    ) {
        let mut s = InMemorySubstrate::from_balances(initial);
        let supply_before = s.total_supply();
        let root_before = s.state_root();
        match s.apply_intent_with_fee(&intent, fee.as_ref()) {
            Ok(()) => {
                prop_assert_eq!(
                    s.total_supply(),
                    supply_before,
                    "successful sponsored intent must conserve supply",
                );
            }
            Err(_) => {
                prop_assert_eq!(
                    s.state_root(),
                    root_before,
                    "a failed intent + fee unit must leave state unchanged",
                );
                prop_assert_eq!(s.total_supply(), supply_before);
            }
        }
    }
}
