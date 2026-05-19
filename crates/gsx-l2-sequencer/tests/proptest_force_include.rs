//! Property tests for the force-include daemon evaluator
//! (Track G G3 / Phase 1.3, #103).
//!
//! Default 256 cases under CI; sprint close runs
//! `PROPTEST_CASES=10000 cargo test --release -p gsx-l2-sequencer`.

use std::collections::{BTreeMap, BTreeSet};

use gsx_l2_sequencer::force_include::{
    evaluate, DaemonAction, EvaluateInput, ObligationId, ObligationSnapshot, ObligationStatus,
    TxHash, EJECTION_WINDOW_L1_BLOCKS,
};
use proptest::prelude::*;

fn status_strategy() -> impl Strategy<Value = ObligationStatus> {
    prop_oneof![
        Just(ObligationStatus::Pending),
        Just(ObligationStatus::Honored),
        Just(ObligationStatus::Slashed),
    ]
}

fn obligation_strategy() -> impl Strategy<Value = ObligationSnapshot> {
    (any::<u8>(), 0u64..=1_000_000, status_strategy()).prop_map(|(seed, deadline, status)| {
        ObligationSnapshot {
            tx_hash: [seed; 32],
            deadline_l1_height: deadline,
            status,
        }
    })
}

/// 16-slot obligation map. ids drawn from u8 so the test
/// corpus is small enough to keep per-case runtime predictable
/// while still exercising the multi-obligation paths.
fn obligation_map_strategy() -> impl Strategy<Value = BTreeMap<ObligationId, ObligationSnapshot>> {
    prop::collection::vec((any::<u8>(), obligation_strategy()), 0..=16).prop_map(|entries| {
        let mut map = BTreeMap::new();
        for (id_seed, ob) in entries {
            map.insert([id_seed; 32], ob);
        }
        map
    })
}

fn committed_tx_strategy() -> impl Strategy<Value = BTreeSet<TxHash>> {
    prop::collection::vec(any::<u8>(), 0..=16)
        .prop_map(|seeds| seeds.into_iter().map(|s| [s; 32]).collect())
}

fn ejected_set_strategy() -> impl Strategy<Value = BTreeSet<ObligationId>> {
    prop::collection::vec(any::<u8>(), 0..=16)
        .prop_map(|seeds| seeds.into_iter().map(|s| [s; 32]).collect())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// **Determinism**: evaluate is a pure function. Two runs
    /// with structurally identical input produce equal output.
    ///
    /// Load-bearing: the daemon may run on multiple operators
    /// for redundancy; they must agree on which Intents to
    /// submit (only one will land — the substrate de-dupes).
    /// If evaluate were non-deterministic, two operators could
    /// produce divergent action lists, wasting RPC bandwidth
    /// and potentially racing to misclassify an obligation.
    #[test]
    fn evaluate_is_deterministic(
        obligations in obligation_map_strategy(),
        committed in committed_tx_strategy(),
        current_l1_height in 0u64..=2_000_000,
        ejected in ejected_set_strategy(),
        ejector_seed in any::<u8>(),
    ) {
        let ejector = [ejector_seed; 20];
        let input1 = EvaluateInput {
            obligations: &obligations,
            committed_batch_tx_hashes: &committed,
            current_l1_height,
            ejected_obligations: &ejected,
            ejector,
        };
        let input2 = input1.clone();
        prop_assert_eq!(evaluate(input1), evaluate(input2));
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// **No double-action per obligation**: for any single
    /// `obligation_id` in the map, evaluate emits AT MOST ONE
    /// action across the three passes — never both honor and
    /// slash, never both slash and eject in the same tick.
    ///
    /// This captures the lifecycle-transitions-are-one-way
    /// invariant on the daemon side. The substrate enforces it
    /// at apply time too; emitting both would just waste an
    /// RPC, but the daemon is the right place to suppress.
    #[test]
    fn at_most_one_action_per_obligation(
        obligations in obligation_map_strategy(),
        committed in committed_tx_strategy(),
        current_l1_height in 0u64..=2_000_000,
        ejected in ejected_set_strategy(),
        ejector_seed in any::<u8>(),
    ) {
        let actions = evaluate(EvaluateInput {
            obligations: &obligations,
            committed_batch_tx_hashes: &committed,
            current_l1_height,
            ejected_obligations: &ejected,
            ejector: [ejector_seed; 20],
        });

        let mut seen = BTreeSet::new();
        for action in &actions {
            // DaemonAction is #[non_exhaustive]; a wildcard
            // future-proofs the match against added variants
            // without breaking the daemon test corpus.
            let id = match action {
                DaemonAction::MarkHonored { obligation_id } => *obligation_id,
                DaemonAction::SlashMissedForceInclude { obligation_id } => *obligation_id,
                DaemonAction::EjectSequencer { obligation_id, .. } => *obligation_id,
                _ => continue,
            };
            prop_assert!(
                seen.insert(id),
                "obligation {:?} received more than one action: {:?}",
                id,
                actions
            );
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// **Honor-wins-tie**: if an obligation is both Pending and
    /// past its deadline AND its tx_hash is in the committed
    /// set, evaluate MUST emit `MarkHonored`, never
    /// `SlashMissedForceInclude`.
    ///
    /// Mirror of `force_include_lifecycle_is_one_way` in
    /// `crates/gsx-execution/tests/proptest_l2.rs`, but on the
    /// daemon side: the daemon must give the sequencer the
    /// benefit of the doubt when both transitions are eligible.
    /// The substrate would reject the slash anyway, but
    /// emitting honor is the bond-preserving move.
    #[test]
    fn pending_with_committed_tx_never_slashes(
        obligations in obligation_map_strategy(),
        committed in committed_tx_strategy(),
        current_l1_height in 0u64..=2_000_000,
        ejected in ejected_set_strategy(),
        ejector_seed in any::<u8>(),
    ) {
        let actions = evaluate(EvaluateInput {
            obligations: &obligations,
            committed_batch_tx_hashes: &committed,
            current_l1_height,
            ejected_obligations: &ejected,
            ejector: [ejector_seed; 20],
        });

        for (id, ob) in &obligations {
            if ob.status == ObligationStatus::Pending
                && committed.contains(&ob.tx_hash)
            {
                let has_slash = actions.iter().any(|a| {
                    matches!(a, DaemonAction::SlashMissedForceInclude { obligation_id } if obligation_id == id)
                });
                prop_assert!(
                    !has_slash,
                    "obligation {:?} was committed but also slashed",
                    id
                );
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// **No-op on terminal states + already-ejected**: evaluate
    /// must NOT emit any action targeting:
    ///   - an obligation that's already `Honored` (terminal)
    ///   - a `Slashed` obligation listed in `ejected_obligations`
    ///   - a `Slashed` obligation whose deadline hasn't yet aged
    ///     past `EJECTION_WINDOW_L1_BLOCKS`
    ///
    /// Together these prove the no-redundant-RPC property:
    /// the daemon never asks the substrate to re-apply a
    /// transition that's already been applied or isn't yet
    /// eligible.
    #[test]
    fn never_acts_on_terminal_or_ineligible_states(
        obligations in obligation_map_strategy(),
        committed in committed_tx_strategy(),
        current_l1_height in 0u64..=2_000_000,
        ejected in ejected_set_strategy(),
        ejector_seed in any::<u8>(),
    ) {
        let actions = evaluate(EvaluateInput {
            obligations: &obligations,
            committed_batch_tx_hashes: &committed,
            current_l1_height,
            ejected_obligations: &ejected,
            ejector: [ejector_seed; 20],
        });

        for action in &actions {
            // DaemonAction is #[non_exhaustive]; a wildcard
            // future-proofs the match against added variants
            // without breaking the daemon test corpus.
            let id = match action {
                DaemonAction::MarkHonored { obligation_id } => *obligation_id,
                DaemonAction::SlashMissedForceInclude { obligation_id } => *obligation_id,
                DaemonAction::EjectSequencer { obligation_id, .. } => *obligation_id,
                _ => continue,
            };
            let ob = obligations.get(&id).expect("action targets a known obligation");

            // No action ever targets a Honored obligation.
            prop_assert_ne!(
                ob.status,
                ObligationStatus::Honored,
                "action {:?} targets a Honored obligation",
                action
            );

            // EjectSequencer requires Slashed status + past
            // ejection window + not already ejected.
            if let DaemonAction::EjectSequencer { obligation_id, .. } = action {
                prop_assert_eq!(ob.status, ObligationStatus::Slashed);
                prop_assert!(!ejected.contains(obligation_id));
                let eject_at = ob
                    .deadline_l1_height
                    .saturating_add(EJECTION_WINDOW_L1_BLOCKS);
                prop_assert!(current_l1_height >= eject_at);
            }

            // SlashMissedForceInclude requires Pending +
            // current > deadline + not committed (the
            // honor-wins-tie property is its inverse).
            if let DaemonAction::SlashMissedForceInclude { .. } = action {
                prop_assert_eq!(ob.status, ObligationStatus::Pending);
                prop_assert!(current_l1_height > ob.deadline_l1_height);
                prop_assert!(!committed.contains(&ob.tx_hash));
            }

            // MarkHonored requires Pending + tx_hash committed.
            if let DaemonAction::MarkHonored { .. } = action {
                prop_assert_eq!(ob.status, ObligationStatus::Pending);
                prop_assert!(committed.contains(&ob.tx_hash));
            }
        }
    }
}
