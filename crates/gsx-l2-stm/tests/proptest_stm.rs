//! Property tests for the L2 STM (Track G G1 / Phase 1.1).
//!
//! Four invariants the SP1 guest must preserve when it lands.
//! All checks run against the native reference STM today; the
//! guest reuses the same `execute_batch` + `to_public_inputs`
//! functions, so any regression here would also regress the
//! guest's output.
//!
//! Default 256 cases under CI; sprint close runs
//! `PROPTEST_CASES=10000 cargo test --release -p gsx-l2-stm`.

use std::collections::BTreeMap;

use gsx_l2_stm::{
    compute_state_root, encode_da_blob, execute_batch, to_public_inputs, Account, Address,
    BatchInput, BatchTransaction, StmError,
};
use gsx_l2_verifier_precompile::{public_inputs as pi_offsets, L2_PUBLIC_INPUTS_BYTES};
use proptest::prelude::*;

/// 8 distinct address slots, biased to non-zero first byte so
/// the state-root's "skip default-zero entries" rule doesn't
/// collide with the test's pre-funding.
fn addr_strategy() -> impl Strategy<Value = Address> {
    (1u8..=8u8).prop_map(|seed| {
        let mut a = [0u8; 20];
        a[0] = seed;
        a
    })
}

/// A pre-funded ledger built from a small Vec of (address,
/// balance) pairs. Per-address pairs are summed so the same
/// address can appear multiple times in the strategy without
/// blowing up the per-test runtime.
fn ledger_strategy() -> impl Strategy<Value = BTreeMap<Address, Account>> {
    prop::collection::vec((addr_strategy(), 1u128..=1_000_000), 0..=8).prop_map(|entries| {
        let mut ledger = BTreeMap::new();
        for (addr, balance) in entries {
            let acct = ledger.entry(addr).or_insert_with(Account::default);
            acct.balance = acct.balance.saturating_add(balance);
        }
        ledger
    })
}

/// A Transfer tx. Picks from + to from the ledger's address
/// space; nonce defaults to 0 (the test passes pre-funded
/// accounts whose nonce starts at 0).
fn tx_strategy() -> impl Strategy<Value = BatchTransaction> {
    (addr_strategy(), addr_strategy(), 1u128..=10_000).prop_map(|(from, to, amount)| {
        BatchTransaction::Transfer {
            from,
            to,
            amount,
            nonce: 0,
        }
    })
}

/// Apply a sequence of txs to a fresh ledger and return the
/// post-state. Skips txs that would reject (e.g. wrong nonce,
/// insufficient balance) — proptests focus on accepted paths.
fn apply_until_done(
    initial_ledger: BTreeMap<Address, Account>,
    txs: &[BatchTransaction],
) -> (BatchInput, Vec<BatchTransaction>) {
    // Filter to txs that will succeed AGAINST THE INITIAL LEDGER.
    // We don't run them yet — we only filter so the batch ends up
    // with txs we expect to apply. The actual execution then
    // runs the same set via execute_batch.
    let mut ledger_preview = initial_ledger.clone();
    let mut applied = Vec::new();
    for tx in txs {
        // BatchTransaction is #[non_exhaustive], so we need a
        // wildcard arm to satisfy the borrow checker — future
        // variants (revm Call/Create) won't compile-break this
        // helper, they just get skipped from the test corpus.
        let (from, to, amount, nonce) = match tx {
            BatchTransaction::Transfer {
                from,
                to,
                amount,
                nonce,
            } => (from, to, amount, nonce),
            _ => continue,
        };
        // Run the tx on a per-tx scratch clone and commit only on
        // full success. This mirrors `execute_batch`'s own
        // clone-then-apply_tx sequence exactly (debit → nonce++ →
        // credit, on the SAME account for a self-transfer), so the
        // filter accepts precisely the txs `execute_batch` will apply.
        //
        // The previous version mutated `ledger_preview` (debit +
        // nonce++) and then `continue`d on credit-overflow WITHOUT
        // rolling back, leaving the preview carrying a tx absent from
        // `applied`. Later txs were then filtered against a phantom
        // state, diverging from `execute_batch` and crashing the
        // `.expect("filtered batch must succeed")`. Discarding the
        // scratch on the overflow path makes it a true no-op. (#257)
        let mut scratch = ledger_preview.clone();
        let from_acct = scratch.entry(*from).or_default();
        if from_acct.nonce != *nonce || from_acct.balance < *amount {
            continue;
        }
        from_acct.balance -= amount;
        from_acct.nonce += 1;
        let to_acct = scratch.entry(*to).or_default();
        let Some(sum) = to_acct.balance.checked_add(*amount) else {
            continue; // scratch dropped -> ledger_preview untouched
        };
        to_acct.balance = sum;
        ledger_preview = scratch;
        applied.push(tx.clone());
    }

    let input = BatchInput {
        prev_l2_state_root: compute_state_root(&initial_ledger),
        batch_id: 1,
        da_blob: encode_da_blob(1, &applied),
        prev_l1_state_root: [0u8; 32],
        l2_chain_id_hash: [0u8; 32],
        l1_anchor_height: 0,
        range_vk_commitment: [0u8; 32],
        ledger: initial_ledger,
    };
    (input, applied)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// **Determinism**: `execute_batch(input)` is a pure function.
    /// Two independent runs with the same `BatchInput` produce
    /// byte-identical `BatchOutput`s — state root, da_commitment,
    /// confidential_root, and post-batch ledger all match.
    ///
    /// This is the load-bearing claim for proof generation:
    /// if execute_batch wasn't deterministic, two provers
    /// would produce different proofs for the same batch.
    #[test]
    fn execute_batch_is_deterministic(
        ledger in ledger_strategy(),
        txs in prop::collection::vec(tx_strategy(), 0..=8),
    ) {
        let (input, _) = apply_until_done(ledger, &txs);
        let out1 = execute_batch(&input).expect("filtered batch must succeed");
        let out2 = execute_batch(&input).expect("filtered batch must succeed");
        prop_assert_eq!(&out1.new_l2_state_root, &out2.new_l2_state_root);
        prop_assert_eq!(&out1.da_commitment, &out2.da_commitment);
        prop_assert_eq!(&out1.confidential_root, &out2.confidential_root);
        prop_assert_eq!(&out1.ledger, &out2.ledger);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// **Empty-batch identity**: an empty tx stream produces:
    /// (a) the same state root as the input ledger,
    /// (b) `da_commitment = BLAKE3(u64::BE(batch_id))` (just
    ///     the 8-byte header), and
    /// (c) the input ledger unchanged.
    ///
    /// The chain commits empty batches when no L2 traffic is
    /// in flight; the STM must be a no-op for them.
    #[test]
    fn empty_batch_is_identity(
        ledger in ledger_strategy(),
        batch_id in 0u64..u64::MAX,
    ) {
        let input = BatchInput {
            prev_l2_state_root: compute_state_root(&ledger),
            batch_id,
            da_blob: encode_da_blob(batch_id, &[]),
            prev_l1_state_root: [0u8; 32],
            l2_chain_id_hash: [0u8; 32],
            l1_anchor_height: 0,
            range_vk_commitment: [0u8; 32],
            ledger: ledger.clone(),
        };
        let out = execute_batch(&input).unwrap();
        prop_assert_eq!(out.new_l2_state_root, input.prev_l2_state_root);
        prop_assert_eq!(out.ledger, ledger);
        prop_assert_eq!(
            out.da_commitment,
            *blake3::hash(&batch_id.to_be_bytes()).as_bytes()
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// **Conservation of supply**: applying a batch never changes
    /// the total sum of balances. Transfers move balance but
    /// don't mint or burn.
    ///
    /// This is the L2 analog of the L1 substrate's
    /// `total_supply_preserved` proptest. Failure here would
    /// mean the L2 mints (or burns) value out of thin air —
    /// directly compromising the bridge invariant on L1.
    #[test]
    fn supply_is_preserved(
        ledger in ledger_strategy(),
        txs in prop::collection::vec(tx_strategy(), 0..=8),
    ) {
        let supply_before: u128 = ledger.values().map(|a| a.balance).sum();
        let (input, _) = apply_until_done(ledger, &txs);
        let out = execute_batch(&input).expect("filtered batch must succeed");
        let supply_after: u128 = out.ledger.values().map(|a| a.balance).sum();
        prop_assert_eq!(supply_after, supply_before);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// **Public-input layout invariant**: `to_public_inputs` lays
    /// every field at the exact byte offset
    /// `gsx_l2_verifier_precompile::public_inputs::*` specifies.
    /// Property-style test (not just a fixed fixture) so a
    /// silent reorder of the BatchInput/BatchOutput fields can't
    /// slip through — every random byte we put into a field
    /// surfaces at the right offset.
    #[test]
    fn public_inputs_layout_round_trips(
        prev_l2 in prop::array::uniform32(0u8..=255u8),
        new_l2 in prop::array::uniform32(0u8..=255u8),
        batch_id in 0u64..u64::MAX,
        da_commit in prop::array::uniform32(0u8..=255u8),
        l1_height in 0u64..u64::MAX,
        range_vk in prop::array::uniform32(0u8..=255u8),
        prev_l1 in prop::array::uniform32(0u8..=255u8),
        chain_id in prop::array::uniform32(0u8..=255u8),
        conf_root in prop::array::uniform32(0u8..=255u8),
    ) {
        let input = BatchInput {
            prev_l2_state_root: prev_l2,
            batch_id,
            da_blob: vec![],
            prev_l1_state_root: prev_l1,
            l2_chain_id_hash: chain_id,
            l1_anchor_height: l1_height,
            range_vk_commitment: range_vk,
            ledger: BTreeMap::new(),
        };
        let output = gsx_l2_stm::BatchOutput {
            new_l2_state_root: new_l2,
            da_commitment: da_commit,
            confidential_root: conf_root,
            ledger: BTreeMap::new(),
        };
        let pi = to_public_inputs(&input, &output);
        prop_assert_eq!(pi.len(), L2_PUBLIC_INPUTS_BYTES);
        prop_assert_eq!(
            &pi[pi_offsets::PREV_L2_STATE_ROOT_OFFSET..pi_offsets::PREV_L2_STATE_ROOT_OFFSET + 32],
            prev_l2
        );
        prop_assert_eq!(
            &pi[pi_offsets::NEW_L2_STATE_ROOT_OFFSET..pi_offsets::NEW_L2_STATE_ROOT_OFFSET + 32],
            new_l2
        );
        prop_assert_eq!(
            u64::from_be_bytes(pi[pi_offsets::BATCH_ID_OFFSET..pi_offsets::BATCH_ID_OFFSET + 8].try_into().unwrap()),
            batch_id
        );
        prop_assert_eq!(
            &pi[pi_offsets::DA_COMMITMENT_OFFSET..pi_offsets::DA_COMMITMENT_OFFSET + 32],
            da_commit
        );
        prop_assert_eq!(
            u64::from_be_bytes(pi[pi_offsets::L1_ANCHOR_HEIGHT_OFFSET..pi_offsets::L1_ANCHOR_HEIGHT_OFFSET + 8].try_into().unwrap()),
            l1_height
        );
        prop_assert_eq!(
            &pi[pi_offsets::RANGE_VK_COMMITMENT_OFFSET..pi_offsets::RANGE_VK_COMMITMENT_OFFSET + 32],
            range_vk
        );
        prop_assert_eq!(
            &pi[pi_offsets::PREV_L1_STATE_ROOT_OFFSET..pi_offsets::PREV_L1_STATE_ROOT_OFFSET + 32],
            prev_l1
        );
        prop_assert_eq!(
            &pi[pi_offsets::L2_CHAIN_ID_HASH_OFFSET..pi_offsets::L2_CHAIN_ID_HASH_OFFSET + 32],
            chain_id
        );
        prop_assert_eq!(
            &pi[pi_offsets::CONFIDENTIAL_ROOT_OFFSET..pi_offsets::CONFIDENTIAL_ROOT_OFFSET + 32],
            conf_root
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// **Pre-state-root soundness gate**: any `BatchInput` whose
    /// `prev_l2_state_root` does NOT equal
    /// `compute_state_root(&input.ledger)` is rejected with
    /// `PreStateRootMismatch` BEFORE any tx is applied.
    ///
    /// This is the load-bearing soundness check for the SP1 guest:
    /// `to_public_inputs` commits the caller-provided
    /// `prev_l2_state_root` verbatim, so without this gate a
    /// malicious prover could pair an arbitrary ledger witness with
    /// a claimed root and produce a "valid" proof for an unanchored
    /// pre-state.
    #[test]
    fn pre_state_root_mismatch_is_rejected(
        ledger in ledger_strategy(),
        bogus_root in prop::array::uniform32(0u8..=255u8),
        txs in prop::collection::vec(tx_strategy(), 0..=8),
    ) {
        let actual_root = compute_state_root(&ledger);
        // Skip cases where the random bytes happen to collide with
        // the real root — astronomically rare for a 256-bit hash
        // but proptest can still trip it under shrinking.
        prop_assume!(bogus_root != actual_root);

        let input = BatchInput {
            prev_l2_state_root: bogus_root,
            batch_id: 1,
            da_blob: encode_da_blob(1, &txs),
            prev_l1_state_root: [0u8; 32],
            l2_chain_id_hash: [0u8; 32],
            l1_anchor_height: 0,
            range_vk_commitment: [0u8; 32],
            ledger,
        };
        match execute_batch(&input) {
            Err(StmError::PreStateRootMismatch { actual, claimed }) => {
                prop_assert_eq!(actual, actual_root);
                prop_assert_eq!(claimed, bogus_root);
            }
            other => prop_assert!(
                false,
                "expected PreStateRootMismatch, got {:?}",
                other
            ),
        }
    }
}
