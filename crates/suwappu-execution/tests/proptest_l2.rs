//! Track G L2-surface exit-gate property tests (Phase 1.4 per
//! `~/.claude/plans/validated-prancing-curry.md`).
//!
//! The substrate's L2 Intent variants (L1Lock, L2BurnProven,
//! L2ForceInclude, MarkForceIncludeHonored, SlashSequencer,
//! SetL2VerifyingKey) ship with apply arms and per-variant unit
//! tests inline in `substrate.rs`. What was missing was a 10k-case
//! property gate of the cross-variant invariants — the kind of
//! check that catches subtle state-shape regressions a unit test
//! won't find.
//!
//! The four properties below are the substrate-level invariants
//! that the rest of the L2 surface (sequencer, prover, bridge
//! relayer, force-include daemon) all depend on.
//!
//! Default 256 cases under CI; sprint close runs
//! `PROPTEST_CASES=10000 cargo test -p suwappu-execution --release`
//! per `SUWAPPUHELPER.md` exit-gate convention.

use suwappu_execution::{
    force_include::{obligation_id, ObligationStatus},
    l2_state, reserved, Address, InMemorySubstrate, Intent, Substrate,
};
use proptest::prelude::*;

// ---- helpers ----------------------------------------------------------------

/// 8 non-reserved address slots. Reserved-address gating rejects
/// `Intent::Transfer` and several L2 intents (e.g. L1Lock's
/// `user_address` cannot be reserved); strategy-level skip avoids
/// fighting that gate inside the proptest body.
fn user_address_strategy() -> impl Strategy<Value = Address> {
    (1u8..=8u8).prop_map(|seed| {
        // Bias the address into the non-reserved tail of the 20-byte
        // space. Reserved addresses are derived via BLAKE3 over a
        // domain tag (see `reserved::*`); any address whose first
        // byte is in 1..=8 with the rest zero is far outside that
        // image and won't collide.
        let mut a = [0u8; 20];
        a[0] = seed;
        a
    })
}

/// Strategy for an L2 chain id hash. Three distinct chains plus
/// the v1 default (`[0; 32]`).
fn chain_id_hash_strategy() -> impl Strategy<Value = [u8; 32]> {
    (0u8..=3u8).prop_map(|seed| {
        let mut a = [0u8; 32];
        a[0] = seed;
        a
    })
}

/// Pair of distinct chain-id hashes. Constructed so `a != b` by
/// derivation, avoiding the `prop_assume!` reject-rate cliff when
/// the per-chain isolation test draws two of the same seed.
fn distinct_chain_id_hash_pair_strategy() -> impl Strategy<Value = ([u8; 32], [u8; 32])> {
    (0u8..=3u8, 1u8..=3u8).prop_map(|(a_seed, offset)| {
        let b_seed = (a_seed + offset) % 4; // guaranteed != a_seed
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        a[0] = a_seed;
        b[0] = b_seed;
        (a, b)
    })
}

/// Strategy for a non-zero VK hash. SetL2VerifyingKey rejects
/// the all-zeros sentinel.
fn vk_hash_strategy() -> impl Strategy<Value = [u8; 32]> {
    (1u8..=255u8).prop_map(|seed| [seed; 32])
}

// ---- Property 1: deposit round-trip preserves bridge invariant ------------

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        .. ProptestConfig::default()
    })]

    /// **Bridge accounting invariant**: after any sequence of
    /// successful `Intent::L1Lock` applies, the balance at
    /// `reserved::bridge_escrow_address()` equals the sum of the
    /// amounts deposited.
    ///
    /// This is the load-bearing safety claim of the L1→L2 deposit
    /// path: the escrow is a reified ledger of "what the L2 owes
    /// back to L1." Failure here means the bridge could mint or
    /// burn balance silently.
    #[test]
    fn bridge_escrow_equals_sum_of_unwithdrawn_locks(
        deposits in prop::collection::vec(
            (user_address_strategy(), user_address_strategy(), 1u128..=1_000_000),
            0..=20,
        )
    ) {
        // Fund each user with enough to cover all their deposits.
        // Build initial balances by summing per-user deposit totals.
        // (Two-statement form: Rust evaluates the RHS of `a = b`
        // BEFORE the LHS, so `*entry.or_insert(0) = funding[user] + …`
        // panics on first insert because the index lookup precedes
        // the entry insertion.)
        use std::collections::HashMap;
        let mut funding: HashMap<Address, u128> = HashMap::new();
        for (user, _recipient, amount) in &deposits {
            let slot = funding.entry(*user).or_insert(0);
            *slot = slot.saturating_add(*amount);
        }
        let mut s = InMemorySubstrate::from_balances(
            funding.iter().map(|(a, b)| (*a, *b)),
        );

        let mut expected_escrow: u128 = 0;
        for (user, recipient, amount) in deposits {
            let intent = Intent::L1Lock {
                user_address: user,
                l2_recipient: recipient,
                amount,
                asset_id: None,
            };
            match s.apply_intent(&intent) {
                Ok(()) => expected_escrow = expected_escrow.saturating_add(amount),
                Err(_) => {
                    // L1Lock can legitimately fail (e.g.
                    // insufficient balance if a user re-debits
                    // beyond their funded amount). In that case
                    // the escrow MUST be unchanged.
                }
            }
            prop_assert_eq!(
                s.balance(&reserved::bridge_escrow_address()),
                expected_escrow,
                "bridge escrow drifted from the running sum of accepted locks"
            );
        }
    }
}

// ---- Property 2: L2BurnProven gate rejects uncommitted batches ------------

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, .. ProptestConfig::default() })]

    /// **Batch-commit gate**: `Intent::L2BurnProven` against any
    /// `(l2_chain_id_hash, batch_id)` that has not been committed
    /// via a prior `Intent::CommitL2StateRoot` MUST reject with
    /// `L2BatchNotCommitted`, and the bridge escrow balance MUST
    /// be unchanged on rejection.
    ///
    /// Without this gate a caller with a plausible-shaped
    /// merkle_path could drain the escrow against an unproven
    /// batch_id. This proptest exercises the negative case;
    /// positive (real-proof) coverage requires the
    /// `suwappu-l2-stm` SP1 circuit (Phase 1.1).
    #[test]
    fn burn_proven_rejects_when_batch_not_committed(
        recipient in user_address_strategy(),
        depositor in user_address_strategy(),
        deposit_amt in 1u128..=1_000_000,
        burn_amt in 1u128..=1_000_000,
        batch_id in 0u64..u64::MAX,
        chain_id_hash in chain_id_hash_strategy(),
        merkle_path_len in 0usize..=128,
    ) {
        // Pre-fund the escrow via a legitimate L1Lock so we can
        // verify the burn doesn't drain it.
        let mut s = InMemorySubstrate::from_balances([(depositor, deposit_amt)]);
        s.apply_intent(&Intent::L1Lock {
            user_address: depositor,
            l2_recipient: recipient,
            amount: deposit_amt,
            asset_id: None,
        })
        .unwrap();
        let escrow_before = s.balance(&reserved::bridge_escrow_address());
        prop_assert_eq!(escrow_before, deposit_amt);

        // Submit an L2BurnProven for an arbitrary batch_id that
        // was never committed.
        let merkle_path = vec![0xABu8; merkle_path_len];
        let err = s
            .apply_intent(&Intent::L2BurnProven {
                batch_id,
                recipient,
                amount: burn_amt,
                merkle_path,
                path_directions: vec![],
                asset_id: None,
                l2_chain_id_hash: chain_id_hash,
            })
            .expect_err("burn against uncommitted batch must reject");

        // Match on the discriminant via Display; the error type's
        // own Eq impl is variant-specific and there's no public
        // matches!-friendly accessor today.
        let s_err = format!("{err:?}");
        prop_assert!(
            s_err.contains("L2BatchNotCommitted"),
            "expected L2BatchNotCommitted, got: {s_err}"
        );

        // Escrow must be unchanged on rejection (atomicity per
        // the Substrate trait contract).
        prop_assert_eq!(
            s.balance(&reserved::bridge_escrow_address()),
            escrow_before,
            "L2BurnProven rejection leaked escrow balance"
        );
    }
}

// ---- Property 3: force-include obligation lifecycle is one-way ------------

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, .. ProptestConfig::default() })]

    /// **Obligation lifecycle**: `Pending → {Honored, Slashed}` is
    /// a one-way transition. After an obligation has transitioned
    /// out of `Pending`, neither `Intent::MarkForceIncludeHonored`
    /// nor `Intent::SlashSequencer` may flip it again — both
    /// reject with `ForceIncludeNotPending`.
    ///
    /// This is the replay-defense gate for the force-include
    /// surface. Double-honoring or double-slashing would let the
    /// sequencer (or a snitch) extract bond rewards twice.
    #[test]
    fn force_include_lifecycle_is_one_way(
        tx_seed in 0u8..=255u8,
        deadline in 0u64..u64::MAX,
        submitter in user_address_strategy(),
        l2_nonce in 0u64..u64::MAX,
        // 0 = transition to Honored, 1 = transition to Slashed
        path in 0u8..=1u8,
    ) {
        let mut s = InMemorySubstrate::new();
        let tx = vec![tx_seed; 32]; // small fixed-size tx body

        // Register the obligation.
        s.apply_intent(&Intent::L2ForceInclude {
            tx: tx.clone(),
            deadline_l1_height: deadline,
            submitter,
            l2_nonce,
        })
        .unwrap();
        let id = obligation_id(&tx, deadline, &submitter, l2_nonce);

        // Re-registering the same (tx, deadline, submitter, nonce)
        // must reject — replay defense at insert time.
        let dup_err = s.apply_intent(&Intent::L2ForceInclude {
            tx: tx.clone(),
            deadline_l1_height: deadline,
            submitter,
            l2_nonce,
        });
        prop_assert!(
            format!("{:?}", dup_err.unwrap_err())
                .contains("ForceIncludeAlreadyRegistered"),
            "duplicate L2ForceInclude must reject"
        );

        // Transition the obligation out of Pending.
        if path == 0 {
            s.apply_intent(&Intent::MarkForceIncludeHonored { obligation_id: id })
                .unwrap();
            prop_assert_eq!(
                s.force_include_obligation(&id).unwrap().status,
                ObligationStatus::Honored
            );
        } else {
            s.apply_intent(&Intent::SlashSequencer {
                reason: suwappu_execution::substrate::SlashReason::MissedForceInclude,
                intent_hash: id,
            })
            .unwrap();
            prop_assert_eq!(
                s.force_include_obligation(&id).unwrap().status,
                ObligationStatus::Slashed
            );
        }

        // From here, BOTH MarkForceIncludeHonored AND
        // SlashSequencer against the same id must reject with
        // ForceIncludeNotPending.
        let honor_again = s.apply_intent(&Intent::MarkForceIncludeHonored {
            obligation_id: id,
        });
        prop_assert!(
            format!("{:?}", honor_again.unwrap_err()).contains("ForceIncludeNotPending"),
            "second honor on non-Pending obligation must reject"
        );

        let slash_again = s.apply_intent(&Intent::SlashSequencer {
            reason: suwappu_execution::substrate::SlashReason::MissedForceInclude,
            intent_hash: id,
        });
        prop_assert!(
            format!("{:?}", slash_again.unwrap_err()).contains("ForceIncludeNotPending"),
            "second slash on non-Pending obligation must reject"
        );
    }
}

// ---- Property 4: VK rotation is per-chain-scoped --------------------------

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, .. ProptestConfig::default() })]

    /// **Per-chain VK isolation**: `Intent::SetL2VerifyingKey { chain_id_hash, .. }`
    /// for chain X must not modify chain Y's pinned VK. Rotating
    /// chain X again only affects X.
    ///
    /// This is the multi-L2 namespacing invariant from IQ-006.
    /// Without it, rotating one chain's VK would invalidate every
    /// other chain's pending CommitL2StateRoots.
    #[test]
    fn set_l2_vk_is_per_chain_scoped(
        chains in distinct_chain_id_hash_pair_strategy(),
        agg_a in vk_hash_strategy(),
        rng_a in vk_hash_strategy(),
        agg_b in vk_hash_strategy(),
        rng_b in vk_hash_strategy(),
        new_agg_a in vk_hash_strategy(),
        new_rng_a in vk_hash_strategy(),
    ) {
        let (chain_a, chain_b) = chains;
        let mut s = InMemorySubstrate::new();

        // Pin VK for chain A.
        s.apply_intent(&Intent::SetL2VerifyingKey {
            chain_id_hash: chain_a,
            new_aggregation_vk: agg_a,
            new_range_commitment: rng_a,
        })
        .unwrap();

        // Pin VK for chain B.
        s.apply_intent(&Intent::SetL2VerifyingKey {
            chain_id_hash: chain_b,
            new_aggregation_vk: agg_b,
            new_range_commitment: rng_b,
        })
        .unwrap();

        // Inspect registry state via read_bytes + l2_state::decode.
        let bytes = s
            .read_bytes(&reserved::l2_registry_address())
            .expect("registry exists after two SetL2VerifyingKey applies");
        let reg = l2_state::decode(&bytes).expect("v2 registry decodes");
        prop_assert_eq!(reg.aggregation_vk_hash(&chain_a), agg_a);
        prop_assert_eq!(reg.range_vk_commitment(&chain_a), rng_a);
        prop_assert_eq!(reg.aggregation_vk_hash(&chain_b), agg_b);
        prop_assert_eq!(reg.range_vk_commitment(&chain_b), rng_b);

        // Rotate chain A; chain B's pin must survive untouched.
        s.apply_intent(&Intent::SetL2VerifyingKey {
            chain_id_hash: chain_a,
            new_aggregation_vk: new_agg_a,
            new_range_commitment: new_rng_a,
        })
        .unwrap();
        let bytes = s.read_bytes(&reserved::l2_registry_address()).unwrap();
        let reg = l2_state::decode(&bytes).unwrap();
        prop_assert_eq!(reg.aggregation_vk_hash(&chain_a), new_agg_a);
        prop_assert_eq!(reg.range_vk_commitment(&chain_a), new_rng_a);
        prop_assert_eq!(reg.aggregation_vk_hash(&chain_b), agg_b);
        prop_assert_eq!(reg.range_vk_commitment(&chain_b), rng_b);
    }
}
