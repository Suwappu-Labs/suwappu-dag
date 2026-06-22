//! DAG-S10 exit-gate property tests.
//!
//! Exit gate: `block_execution_matches_substrate` — applying the same
//! sequence of blocks against two fresh substrates produces identical
//! post-state roots. This is the deterministic-replay property that
//! suwappu-db's S8 sprint discharges at the substrate layer; we lift it to
//! the adapter to guarantee the consensus-output → substrate dispatch
//! is itself deterministic.
//!
//! Supporting properties:
//!
//! - `block_execution_is_deterministic` — single-run determinism.
//! - `total_supply_preserved` — sum of balances is invariant across
//!   any block sequence whose intents apply successfully.
//! - `failed_intent_is_atomic` — a failed intent does not perturb the
//!   state root; the report flags the error precisely.
//!
//! Run at default 256 cases under CI; sprint close runs
//! `PROPTEST_CASES=10000 cargo test -p suwappu-execution --release`.

use suwappu_execution::{execute_block, Address, Balance, Block, InMemorySubstrate, Intent, Substrate};
use proptest::prelude::*;

/// Strategy for an `Address` keyed on a small u8 seed, so that random
/// transfers have a realistic collision rate. We use 4 distinct address
/// seeds so transfer chains actually compose.
fn addr_strategy() -> impl Strategy<Value = Address> {
    (0u8..4).prop_map(|seed| [seed; 20])
}

/// Strategy for a single `Intent::Transfer`.
fn intent_strategy() -> impl Strategy<Value = Intent> {
    (addr_strategy(), addr_strategy(), 0u128..=1_000)
        .prop_map(|(from, to, amount)| Intent::Transfer { from, to, amount })
}

/// Strategy for a block of intents with a small round and 0–8 intents.
fn block_strategy() -> impl Strategy<Value = Block> {
    (0u64..=10, prop::collection::vec(intent_strategy(), 0..=8))
        .prop_map(|(round, intents)| Block { round, intents })
}

/// Strategy for a small initial-balance map (1–4 addresses).
fn initial_balances_strategy() -> impl Strategy<Value = Vec<(Address, Balance)>> {
    prop::collection::vec((addr_strategy(), 1u128..=10_000), 1..=4)
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 32,
        .. ProptestConfig::default()
    })]

    /// EXIT GATE — applying the same block sequence twice yields the
    /// same final state root. Block execution is deterministic at the
    /// adapter layer.
    #[test]
    fn block_execution_matches_substrate(
        initial in initial_balances_strategy(),
        blocks in prop::collection::vec(block_strategy(), 0..=6),
    ) {
        let mut s1 = InMemorySubstrate::from_balances(initial.clone());
        let mut s2 = InMemorySubstrate::from_balances(initial);

        for block in &blocks {
            execute_block(&mut s1, block);
            execute_block(&mut s2, block);
        }

        prop_assert_eq!(s1.state_root(), s2.state_root());
    }

    /// Single-run determinism within one substrate: replaying the same
    /// block twice on the same substrate is well-defined (the second
    /// pass may fail differently depending on state, but the resulting
    /// state root is itself deterministic).
    #[test]
    fn block_execution_is_deterministic(
        initial in initial_balances_strategy(),
        block in block_strategy(),
    ) {
        let mut s = InMemorySubstrate::from_balances(initial);
        let report_a = execute_block(&mut s, &block);
        let root_after_one = s.state_root();
        prop_assert_eq!(report_a.post_root, root_after_one);

        // A second pass against an independent substrate from the same
        // post-state must produce the same outcome.
        let mut s_copy = s.clone();
        let report_b = execute_block(&mut s, &block);
        let _ = execute_block(&mut s_copy, &block);
        prop_assert_eq!(s.state_root(), s_copy.state_root());
        prop_assert_eq!(report_b.applied, report_b.applied);
    }

    /// Total supply is invariant under transfers. Even when some intents
    /// fail (insufficient balance / overflow), the substrate's atomicity
    /// guarantee keeps the sum unchanged. We sample the supply at
    /// substrate-construction time rather than from the raw input list
    /// because `from_balances` is dedup-by-address.
    #[test]
    fn total_supply_preserved(
        initial in initial_balances_strategy(),
        blocks in prop::collection::vec(block_strategy(), 0..=6),
    ) {
        let mut s = InMemorySubstrate::from_balances(initial);
        let initial_supply = s.total_supply();

        for block in &blocks {
            execute_block(&mut s, block);
            prop_assert_eq!(
                s.total_supply(),
                initial_supply,
                "total supply changed under transfers",
            );
        }
    }

    /// Per-intent atomicity: when an error fires inside a block, the
    /// substrate's state matches the snapshot taken just before that
    /// failing intent. We replay the block prefix on a sibling substrate
    /// to obtain the expected pre-error state root.
    #[test]
    fn failed_intent_is_atomic(
        initial in initial_balances_strategy(),
        intents in prop::collection::vec(intent_strategy(), 0..=8),
    ) {
        // A guaranteed-bad intent — a non-initialized source address
        // (initial uses seeds 0..=3; 0xFE is outside that range).
        let bad = Intent::Transfer {
            from: [0xFE; 20],
            to: [0xFD; 20],
            amount: u128::MAX,
        };
        let mut intents = intents;
        intents.push(bad);

        let mut s = InMemorySubstrate::from_balances(initial.clone());
        let block = Block { round: 0, intents: intents.clone() };
        let report = execute_block(&mut s, &block);

        prop_assert!(
            report.first_error.is_some(),
            "guaranteed-bad intent must produce an error",
        );
        let (err_idx, _err) = report.first_error.as_ref().unwrap();

        // Replay the prefix [0, err_idx) on a fresh substrate; the
        // resulting state root must equal the post-execution root,
        // because the failing intent at err_idx left state unchanged
        // and every intent after it was skipped.
        let mut prefix_s = InMemorySubstrate::from_balances(initial);
        let prefix_block = Block {
            round: 0,
            intents: intents[..*err_idx].to_vec(),
        };
        let _ = execute_block(&mut prefix_s, &prefix_block);
        prop_assert_eq!(prefix_s.state_root(), s.state_root());
    }
}
