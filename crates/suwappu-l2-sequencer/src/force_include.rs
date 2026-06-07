//! Force-include daemon logic (Track G G3 / Phase 1.3, #103).
//!
//! Pure-function evaluator that, given a snapshot of the L1
//! force-include registry + the L2 tx hashes the sequencer has
//! committed since the last evaluation + the current L1 height,
//! produces the list of `Intent`s the daemon should submit.
//!
//! Three lifecycle transitions are emitted as
//! [`DaemonAction`]s (one Intent each):
//!
//! 1. **`Pending → Honored`**: the obligation's `tx_hash`
//!    was committed via `Intent::CommitL2StateRoot` at an L1
//!    height **at or before** the obligation's
//!    `deadline_l1_height`. Daemon emits
//!    `Intent::MarkForceIncludeHonored { obligation_id }`. The
//!    decision is made on the recorded **commit height**, not on
//!    `current_l1_height`: an on-time commit observed late is
//!    still honored.
//! 2. **`Pending → Slashed`**: `current_l1_height >
//!    obligation.deadline_l1_height` AND the obligation
//!    has not been honored (no commit at height ≤ deadline).
//!    Daemon emits `Intent::SlashSequencer { reason:
//!    MissedForceInclude, intent_hash: obligation_id }`.
//!    `current_l1_height` is used here ONLY for slash *timing*,
//!    never for the honor decision.
//! 3. **`Slashed → Ejected`**: a Slashed obligation has aged
//!    past `deadline_l1_height + EJECTION_WINDOW_L1_BLOCKS`
//!    and the daemon hasn't already submitted an ejection
//!    record for it. Daemon emits `Intent::EjectSequencer
//!    { obligation_id, ejector }`.
//!
//! ## Why pure logic (no Tokio, no RPC)
//!
//! Matches the [`crate::lib`] phase-1 discipline. The actual
//! polling loop + RPC submission lives in the Phase 2.2
//! sequencer daemon binary (#105); this module owns only the
//! decision rule. Property tests can drive the decision rule
//! through arbitrary obligation maps without standing up a
//! network stack.
//!
//! ## Cross-crate type drift
//!
//! The substrate-side `ForceIncludeObligation` lives in
//! `suwappu-execution::force_include`. We deliberately avoid that
//! dependency to keep `suwappu-l2-sequencer` thin (suwappu-execution
//! pulls suwappu-authority + suwappu-consensus + gsxdb-bridge); the
//! Phase 2.2 daemon binary holds the `From<ForceIncludeObligation>
//! for ObligationSnapshot` conversion at the wiring boundary.
//! [`ObligationSnapshot::ENCODED_BYTES`] pins the wire shape so
//! a substrate-side encoding change triggers an explicit
//! conversion update.

use std::collections::{BTreeMap, BTreeSet};

// `BTreeSet` is still used by `EvaluateInput::ejected_obligations`
// and internal honor bookkeeping; `BTreeMap` now backs both the
// obligation table and the committed-tx-hash → commit-height map.

use serde::{Deserialize, Serialize};

/// 20-byte address (matches `suwappu_execution::substrate::Address`).
pub type Address = [u8; 20];

/// 32-byte obligation identifier (matches
/// `suwappu_execution::force_include::obligation_id`).
pub type ObligationId = [u8; 32];

/// 32-byte L2 transaction hash (matches
/// `suwappu_execution::force_include::tx_hash`).
pub type TxHash = [u8; 32];

/// L1-block window from `deadline_l1_height` after which a
/// `Slashed` obligation is eligible for permissionless
/// sequencer ejection. Track G strategic plan: "after
/// `deadline_l1_height + 10,000 blocks` (≈ 83 min), any
/// address can post a `SequencerEjection` proof". Matches
/// the daemon-side gate noted on `Intent::EjectSequencer`.
pub const EJECTION_WINDOW_L1_BLOCKS: u64 = 10_000;

/// Daemon-side mirror of `suwappu_execution::force_include::ObligationStatus`.
/// Defined locally to keep the dep graph thin; the Phase 2.2
/// wiring layer maps between the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ObligationStatus {
    /// Newly registered; sequencer must include by deadline.
    Pending,
    /// Sequencer included the tx; further slashing rejected.
    Honored,
    /// Sequencer missed the deadline; slashing applied.
    Slashed,
}

/// Daemon-side view of a registered force-include obligation.
/// Mirrors `suwappu_execution::force_include::ForceIncludeObligation`
/// at the fields the daemon needs to decide an action; the
/// `submitter` field is omitted because the daemon doesn't
/// pay the bounty (the substrate's apply arm does).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObligationSnapshot {
    /// BLAKE3 of the L2 tx bytes.
    pub tx_hash: TxHash,
    /// L1 block height at which the obligation expires.
    pub deadline_l1_height: u64,
    /// Current status of the obligation.
    pub status: ObligationStatus,
}

impl ObligationSnapshot {
    /// Wire-shape pin: `tx_hash(32) + deadline_l1_height(8) +
    /// status(1) = 41`. The substrate-side wire shape includes
    /// `submitter(20) + l2_nonce(8)` for an additional 28 B; if
    /// either side changes, the Phase 2.2 conversion fails to
    /// compile and the discrepancy surfaces immediately.
    pub const ENCODED_BYTES: usize = 32 + 8 + 1;
}

/// One Intent the daemon should submit. The Phase 2.2 daemon
/// binary's RPC layer turns these into `Intent` JSON-RPC calls
/// against the L1 substrate.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DaemonAction {
    /// Submit `Intent::MarkForceIncludeHonored { obligation_id }`.
    /// The sequencer included the obligation's tx in a committed
    /// batch before the deadline.
    MarkHonored {
        /// The honored obligation's deterministic id.
        obligation_id: ObligationId,
    },
    /// Submit `Intent::SlashSequencer { reason: MissedForceInclude,
    /// intent_hash: obligation_id }`. The deadline passed without
    /// the sequencer honoring the obligation.
    SlashMissedForceInclude {
        /// The missed obligation's deterministic id; used as
        /// `intent_hash` per the substrate's apply arm.
        obligation_id: ObligationId,
    },
    /// Submit `Intent::EjectSequencer { obligation_id, ejector }`.
    /// A Slashed obligation has aged past the ejection window;
    /// the daemon's operator becomes the next sequencer.
    EjectSequencer {
        /// Obligation justifying the ejection.
        obligation_id: ObligationId,
        /// Daemon operator address (snitch-bounty target).
        ejector: Address,
    },
}

/// Inputs to [`evaluate`]: a single snapshot of L1 state the
/// daemon can act on.
///
/// All four fields are pure reads — three from substrate
/// (`obligations`, `current_l1_height`, `ejected_obligations`),
/// one from the sequencer's own batch-builder log
/// (`committed_batch_tx_hashes`). The daemon should call
/// `evaluate` after every newly committed batch + on a
/// background tick for the deadline / ejection arms.
///
/// ## `committed_batch_tx_hashes` precondition
///
/// This map MUST cover every committed batch tx hash whose
/// origin obligation might still be `Pending` at any past or
/// present L1 height — not just commits "since this daemon last
/// ran" — together with the **L1 height each was committed at**.
/// The honor decision is made on that recorded commit height
/// (`commit_height <= deadline`), so losing it across a restart
/// would let a later tick fire `SlashMissedForceInclude` against
/// an obligation whose tx was already committed *on time*. The
/// substrate rejects the slash on the pending-status check, but
/// only after the daemon has wasted an RPC and (worse) attempted
/// an incorrect adjudication that the network logs as Byzantine.
///
/// The daemon binary owns this precondition. It must persist the
/// committed-hash history (tx_hash → commit height) and replay it
/// at startup, or alternatively scan L1 commits back to the
/// oldest still-Pending obligation's submission height before
/// invoking `evaluate`. The pure logic here trusts the caller's
/// input; see #256/#229 for the daemon-side implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluateInput<'a> {
    /// All registered obligations, decoded from the L1
    /// `force_include_registry_address`. Keyed by obligation_id.
    pub obligations: &'a BTreeMap<ObligationId, ObligationSnapshot>,
    /// Map of L2 tx hashes the sequencer has committed via
    /// `Intent::CommitL2StateRoot` to the **L1 height each was
    /// committed at**, covering all batches relevant to
    /// currently-Pending obligations (see the
    /// `committed_batch_tx_hashes` precondition section above).
    /// Key is `BLAKE3(tx_bytes)` per
    /// `suwappu_execution::force_include::tx_hash`; value is the L1
    /// height observed at commit time. The honor decision keys
    /// off `commit_height <= deadline`, so an on-time commit
    /// observed late (after `current_l1_height` crosses the
    /// deadline) is still honored.
    pub committed_batch_tx_hashes: &'a BTreeMap<TxHash, u64>,
    /// Current L1 block height (from a recent L1 RPC poll). Used
    /// ONLY for slash / eject *timing*, never for the honor
    /// decision (which keys off the recorded commit height).
    pub current_l1_height: u64,
    /// Obligation ids that the daemon (or another snitch) has
    /// already ejected against, decoded from L1's
    /// `ejection_registry_address`. Used to suppress duplicate
    /// `EjectSequencer` emissions — the substrate would reject
    /// the duplicate anyway, but suppressing it saves an RPC.
    pub ejected_obligations: &'a BTreeSet<ObligationId>,
    /// Daemon operator address. Sits in the `ejector` field of
    /// any emitted `EjectSequencer` action so the operator
    /// receives the snitch bounty.
    pub ejector: Address,
}

/// Decide which Intents the daemon should submit given the
/// current L1 state snapshot.
///
/// Deterministic + pure: same input → same output, in
/// obligation_id ascending order. Suitable for property
/// testing across arbitrary input distributions.
///
/// Returns actions in this order:
/// 1. All `MarkHonored` for `Pending` obligations whose `tx_hash`
///    was committed at an L1 height **at or before** the
///    obligation's `deadline_l1_height` (i.e.
///    `committed_batch_tx_hashes.get(tx_hash)` is `Some(h)` with
///    `h <= deadline_l1_height`). Independent of
///    `current_l1_height`.
/// 2. All `SlashMissedForceInclude` for `Pending` obligations
///    whose `deadline_l1_height < current_l1_height` AND were
///    NOT honored this tick (no recorded commit at height ≤
///    deadline).
/// 3. All `EjectSequencer` for `Slashed` obligations past the
///    ejection window AND not yet in `ejected_obligations`.
///
/// **Commit-height honor.** Honor is decided by the recorded
/// **commit height**, never by `current_l1_height`. An on-time
/// commit (height ≤ deadline) is honored even if the daemon only
/// observes it long after the deadline has passed — e.g. after a
/// restart replays the persisted committed-history. This
/// reconciles two earlier approximations:
/// - #268 gated honor on `current_l1_height <= deadline`, which
///   over-slashed: an on-time commit seen late was wrongly
///   slashed (and the daemon's own restart could trigger it).
/// - #280 stored commit heights but its test was defeated by
///   #268's surviving gate.
///
/// A sequencer that genuinely commits a censored tx **after** the
/// deadline still gets slashed: the recorded commit height is
/// `> deadline`, so pass 1 does not honor it and pass 2 fires.
/// The user still receives their tx via the included batch — both
/// events stand: the user got (late) inclusion, the sequencer got
/// slashed for missing the SLA deadline.
///
/// `current_l1_height` is read ONLY by passes 2 and 3 for slash /
/// eject timing.
pub fn evaluate(input: EvaluateInput<'_>) -> Vec<DaemonAction> {
    let mut actions = Vec::new();
    let mut honored_this_tick: BTreeSet<ObligationId> = BTreeSet::new();

    // Pass 1: Pending obligations whose tx_hash was committed at
    // an L1 height at or before the deadline. The honor decision
    // is made purely on the recorded commit height — NOT on
    // `current_l1_height` — so an on-time commit observed late
    // (e.g. after a restart) is still honored. `<=` lets a commit
    // at exactly the deadline block count as honor.
    for (id, ob) in input.obligations {
        if ob.status != ObligationStatus::Pending {
            continue;
        }
        if let Some(&commit_height) = input.committed_batch_tx_hashes.get(&ob.tx_hash) {
            if commit_height <= ob.deadline_l1_height {
                actions.push(DaemonAction::MarkHonored { obligation_id: *id });
                honored_this_tick.insert(*id);
            }
        }
    }

    // Pass 2: Pending obligations past deadline that were NOT
    // honored this tick (no commit recorded at height ≤ deadline).
    // `current_l1_height` is the slash *timing* gate only.
    for (id, ob) in input.obligations {
        if ob.status != ObligationStatus::Pending {
            continue;
        }
        if honored_this_tick.contains(id) {
            continue;
        }
        if input.current_l1_height > ob.deadline_l1_height {
            actions.push(DaemonAction::SlashMissedForceInclude { obligation_id: *id });
        }
    }

    // Pass 3: Slashed obligations past the ejection window that
    // haven't been ejected yet.
    for (id, ob) in input.obligations {
        if ob.status != ObligationStatus::Slashed {
            continue;
        }
        if input.ejected_obligations.contains(id) {
            continue;
        }
        let eject_at = ob
            .deadline_l1_height
            .saturating_add(EJECTION_WINDOW_L1_BLOCKS);
        if input.current_l1_height >= eject_at {
            actions.push(DaemonAction::EjectSequencer {
                obligation_id: *id,
                ejector: input.ejector,
            });
        }
    }

    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ob(tx_hash: u8, deadline: u64, status: ObligationStatus) -> ObligationSnapshot {
        ObligationSnapshot {
            tx_hash: [tx_hash; 32],
            deadline_l1_height: deadline,
            status,
        }
    }

    fn id(b: u8) -> ObligationId {
        [b; 32]
    }

    fn no_committed() -> BTreeMap<TxHash, u64> {
        BTreeMap::new()
    }

    fn no_ejected() -> BTreeSet<ObligationId> {
        BTreeSet::new()
    }

    fn ejector() -> Address {
        [0xee; 20]
    }

    #[test]
    fn empty_inputs_produce_no_actions() {
        let actions = evaluate(EvaluateInput {
            obligations: &BTreeMap::new(),
            committed_batch_tx_hashes: &no_committed(),
            current_l1_height: 0,
            ejected_obligations: &no_ejected(),
            ejector: ejector(),
        });
        assert!(actions.is_empty());
    }

    #[test]
    fn pending_with_committed_tx_emits_mark_honored() {
        let mut map = BTreeMap::new();
        map.insert(id(1), ob(0x42, 100, ObligationStatus::Pending));
        // Committed on time (height 50 <= deadline 100).
        let mut committed = BTreeMap::new();
        committed.insert([0x42; 32], 50u64);

        let actions = evaluate(EvaluateInput {
            obligations: &map,
            committed_batch_tx_hashes: &committed,
            current_l1_height: 50,
            ejected_obligations: &no_ejected(),
            ejector: ejector(),
        });
        assert_eq!(
            actions,
            vec![DaemonAction::MarkHonored {
                obligation_id: id(1)
            }]
        );
    }

    #[test]
    fn pending_past_deadline_emits_slash() {
        let mut map = BTreeMap::new();
        map.insert(id(1), ob(0x42, 100, ObligationStatus::Pending));

        let actions = evaluate(EvaluateInput {
            obligations: &map,
            committed_batch_tx_hashes: &no_committed(),
            current_l1_height: 101,
            ejected_obligations: &no_ejected(),
            ejector: ejector(),
        });
        assert_eq!(
            actions,
            vec![DaemonAction::SlashMissedForceInclude {
                obligation_id: id(1)
            }]
        );
    }

    #[test]
    fn pending_at_deadline_exact_does_not_slash() {
        // deadline_l1_height is the LAST acceptable height; the
        // sequencer still has the block at that height to honor.
        let mut map = BTreeMap::new();
        map.insert(id(1), ob(0x42, 100, ObligationStatus::Pending));

        let actions = evaluate(EvaluateInput {
            obligations: &map,
            committed_batch_tx_hashes: &no_committed(),
            current_l1_height: 100,
            ejected_obligations: &no_ejected(),
            ejector: ejector(),
        });
        assert!(actions.is_empty());
    }

    #[test]
    fn late_inclusion_after_deadline_slashes_not_honors() {
        // A sequencer that genuinely commits a censored tx *after*
        // the deadline must not avoid `SlashMissedForceInclude` by
        // virtue of the late inclusion. Under commit-height honor,
        // "late inclusion" means the recorded COMMIT height is past
        // the deadline (200 > deadline 100): pass 1 does not honor
        // it (commit_height > deadline), so pass 2 fires the slash.
        // This is distinct from an on-time commit observed late
        // (see `on_time_commit_observed_late_still_honors`), which
        // is honored — that is exactly the #268 over-slash this fix
        // removes.
        let mut map = BTreeMap::new();
        map.insert(id(1), ob(0x42, 100, ObligationStatus::Pending));
        // Committed LATE: commit height 200 > deadline 100.
        let mut committed = BTreeMap::new();
        committed.insert([0x42; 32], 200u64);

        let actions = evaluate(EvaluateInput {
            obligations: &map,
            committed_batch_tx_hashes: &committed,
            current_l1_height: 200, // past deadline 100
            ejected_obligations: &no_ejected(),
            ejector: ejector(),
        });
        assert_eq!(
            actions,
            vec![DaemonAction::SlashMissedForceInclude {
                obligation_id: id(1)
            }]
        );
    }

    #[test]
    fn on_time_commit_observed_late_still_honors() {
        // The canonical #268-over-slash regression at the pure-logic
        // level: the tx was COMMITTED on time (height 100 <=
        // deadline 100) but the daemon only evaluates much later
        // (current 1500, far past the deadline). Honor keys off the
        // recorded commit height, not current_l1_height, so this
        // must emit MarkHonored and NOT slash.
        let mut map = BTreeMap::new();
        map.insert(id(1), ob(0x42, 100, ObligationStatus::Pending));
        // Committed at exactly the deadline (the LAST acceptable
        // height), then observed long after.
        let mut committed = BTreeMap::new();
        committed.insert([0x42; 32], 100u64);

        let actions = evaluate(EvaluateInput {
            obligations: &map,
            committed_batch_tx_hashes: &committed,
            current_l1_height: 1_500, // far past deadline 100
            ejected_obligations: &no_ejected(),
            ejector: ejector(),
        });
        assert_eq!(
            actions,
            vec![DaemonAction::MarkHonored {
                obligation_id: id(1)
            }]
        );
    }

    #[test]
    fn already_honored_obligation_emits_nothing() {
        let mut map = BTreeMap::new();
        map.insert(id(1), ob(0x42, 100, ObligationStatus::Honored));
        let mut committed = BTreeMap::new();
        committed.insert([0x42; 32], 50u64);

        let actions = evaluate(EvaluateInput {
            obligations: &map,
            committed_batch_tx_hashes: &committed,
            current_l1_height: 200,
            ejected_obligations: &no_ejected(),
            ejector: ejector(),
        });
        assert!(actions.is_empty());
    }

    #[test]
    fn slashed_past_ejection_window_emits_eject() {
        let mut map = BTreeMap::new();
        map.insert(id(1), ob(0x42, 100, ObligationStatus::Slashed));

        let actions = evaluate(EvaluateInput {
            obligations: &map,
            committed_batch_tx_hashes: &no_committed(),
            current_l1_height: 100 + EJECTION_WINDOW_L1_BLOCKS,
            ejected_obligations: &no_ejected(),
            ejector: ejector(),
        });
        assert_eq!(
            actions,
            vec![DaemonAction::EjectSequencer {
                obligation_id: id(1),
                ejector: ejector(),
            }]
        );
    }

    #[test]
    fn slashed_inside_ejection_window_emits_nothing() {
        let mut map = BTreeMap::new();
        map.insert(id(1), ob(0x42, 100, ObligationStatus::Slashed));

        let actions = evaluate(EvaluateInput {
            obligations: &map,
            committed_batch_tx_hashes: &no_committed(),
            current_l1_height: 100 + EJECTION_WINDOW_L1_BLOCKS - 1,
            ejected_obligations: &no_ejected(),
            ejector: ejector(),
        });
        assert!(actions.is_empty());
    }

    #[test]
    fn already_ejected_obligation_does_not_re_emit() {
        let mut map = BTreeMap::new();
        map.insert(id(1), ob(0x42, 100, ObligationStatus::Slashed));
        let mut ejected = BTreeSet::new();
        ejected.insert(id(1));

        let actions = evaluate(EvaluateInput {
            obligations: &map,
            committed_batch_tx_hashes: &no_committed(),
            current_l1_height: u64::MAX,
            ejected_obligations: &ejected,
            ejector: ejector(),
        });
        assert!(actions.is_empty());
    }

    #[test]
    fn multi_obligation_emits_in_ordered_groups() {
        let mut map = BTreeMap::new();
        // id(1): Pending, committed at height ≤ deadline → MarkHonored.
        map.insert(id(1), ob(0x11, 20_000, ObligationStatus::Pending));
        // id(2): Pending, deadline missed → SlashMissedForceInclude.
        map.insert(id(2), ob(0x22, 50, ObligationStatus::Pending));
        // id(3): Slashed past ejection window → EjectSequencer.
        map.insert(id(3), ob(0x33, 10, ObligationStatus::Slashed));
        // id(4): Slashed inside ejection window → no-op.
        map.insert(id(4), ob(0x44, 50_000, ObligationStatus::Slashed));

        // id(1)'s tx committed at height 5_000, comfortably ≤ its
        // deadline 20_000 → honored regardless of current height.
        let mut committed = BTreeMap::new();
        committed.insert([0x11; 32], 5_000u64);

        // current_l1_height: past id(2)'s deadline + past id(3)'s
        // deadline+ejection-window. id(1) is honored by commit
        // height (commit 5_000 ≤ deadline 20_000), so even though
        // current_l1_height is past id(1)'s deadline here the honor
        // path still applies — that is the commit-height semantics.
        let current_l1_height = 10 + EJECTION_WINDOW_L1_BLOCKS + 5;
        let actions = evaluate(EvaluateInput {
            obligations: &map,
            committed_batch_tx_hashes: &committed,
            current_l1_height,
            ejected_obligations: &no_ejected(),
            ejector: ejector(),
        });

        // Pass order: all MarkHonored, then all Slash, then all Eject.
        assert_eq!(
            actions,
            vec![
                DaemonAction::MarkHonored {
                    obligation_id: id(1)
                },
                DaemonAction::SlashMissedForceInclude {
                    obligation_id: id(2)
                },
                DaemonAction::EjectSequencer {
                    obligation_id: id(3),
                    ejector: ejector(),
                },
            ]
        );
    }

    #[test]
    fn evaluate_is_deterministic_across_two_runs() {
        let mut map = BTreeMap::new();
        for i in 0..32u8 {
            let status = match i % 3 {
                0 => ObligationStatus::Pending,
                1 => ObligationStatus::Honored,
                _ => ObligationStatus::Slashed,
            };
            map.insert(id(i), ob(i, i as u64 * 100, status));
        }
        let mut committed = BTreeMap::new();
        committed.insert([3u8; 32], 100u64);
        committed.insert([6u8; 32], 100u64);

        let input1 = EvaluateInput {
            obligations: &map,
            committed_batch_tx_hashes: &committed,
            current_l1_height: 50_000,
            ejected_obligations: &no_ejected(),
            ejector: ejector(),
        };
        let input2 = input1.clone();
        assert_eq!(evaluate(input1), evaluate(input2));
    }

    use proptest::prelude::*;

    proptest! {
        /// Commit-height honor invariant over arbitrary
        /// `(commit_height, deadline, current_l1_height)`:
        ///
        /// For a single `Pending` obligation whose tx is recorded
        /// committed at `commit_height`:
        /// - It is **honored** (`MarkHonored`) iff a commit exists
        ///   at height ≤ deadline (`commit_height <= deadline`),
        ///   regardless of `current_l1_height`.
        /// - It is **slashed** (`SlashMissedForceInclude`) iff
        ///   `current_l1_height > deadline` AND no such on-time
        ///   commit exists (`commit_height > deadline`).
        /// - It is **never both** honored and slashed in one tick.
        ///
        /// `commit_height` is drawn independently of `deadline` so
        /// the `commit_height > deadline` (late-commit) case is
        /// genuinely exercised, not just the on-time case.
        #[test]
        fn commit_height_honor_invariant(
            commit_height in 0u64..4_000,
            deadline in 0u64..4_000,
            current_l1_height in 0u64..8_000,
        ) {
            let oid = id(1);
            let txh = [0x42u8; 32];

            let mut obligations = BTreeMap::new();
            obligations.insert(oid, ob(0x42, deadline, ObligationStatus::Pending));

            let mut committed = BTreeMap::new();
            committed.insert(txh, commit_height);

            let actions = evaluate(EvaluateInput {
                obligations: &obligations,
                committed_batch_tx_hashes: &committed,
                current_l1_height,
                ejected_obligations: &no_ejected(),
                ejector: ejector(),
            });

            let honored = actions
                .iter()
                .any(|a| matches!(a, DaemonAction::MarkHonored { obligation_id } if *obligation_id == oid));
            let slashed = actions
                .iter()
                .any(|a| matches!(a, DaemonAction::SlashMissedForceInclude { obligation_id } if *obligation_id == oid));

            let on_time = commit_height <= deadline;

            // Honored iff an on-time commit exists, independent of
            // current_l1_height.
            prop_assert_eq!(honored, on_time, "honor must key off commit_height <= deadline only");

            // Slashed iff past deadline AND not on-time.
            prop_assert_eq!(
                slashed,
                current_l1_height > deadline && !on_time,
                "slash must require current>deadline AND no on-time commit"
            );

            // Never both for one obligation.
            prop_assert!(!(honored && slashed), "an obligation is never both honored and slashed");
        }
    }
}
