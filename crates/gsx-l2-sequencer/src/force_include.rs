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
//!    appears in a batch the sequencer has already committed
//!    via `Intent::CommitL2StateRoot`. Daemon emits
//!    `Intent::MarkForceIncludeHonored { obligation_id }`.
//! 2. **`Pending → Slashed`**: `current_l1_height >
//!    obligation.deadline_l1_height` AND the obligation
//!    has not been honored. Daemon emits
//!    `Intent::SlashSequencer { reason: MissedForceInclude,
//!    intent_hash: obligation_id }`.
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
//! `gsx-execution::force_include`. We deliberately avoid that
//! dependency to keep `gsx-l2-sequencer` thin (gsx-execution
//! pulls gsx-authority + gsx-consensus + gsxdb-bridge); the
//! Phase 2.2 daemon binary holds the `From<ForceIncludeObligation>
//! for ObligationSnapshot` conversion at the wiring boundary.
//! [`ObligationSnapshot::ENCODED_BYTES`] pins the wire shape so
//! a substrate-side encoding change triggers an explicit
//! conversion update.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// 20-byte address (matches `gsx_execution::substrate::Address`).
pub type Address = [u8; 20];

/// 32-byte obligation identifier (matches
/// `gsx_execution::force_include::obligation_id`).
pub type ObligationId = [u8; 32];

/// 32-byte L2 transaction hash (matches
/// `gsx_execution::force_include::tx_hash`).
pub type TxHash = [u8; 32];

/// L1-block window from `deadline_l1_height` after which a
/// `Slashed` obligation is eligible for permissionless
/// sequencer ejection. Track G strategic plan: "after
/// `deadline_l1_height + 10,000 blocks` (≈ 83 min), any
/// address can post a `SequencerEjection` proof". Matches
/// the daemon-side gate noted on `Intent::EjectSequencer`.
pub const EJECTION_WINDOW_L1_BLOCKS: u64 = 10_000;

/// Daemon-side mirror of `gsx_execution::force_include::ObligationStatus`.
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
/// Mirrors `gsx_execution::force_include::ForceIncludeObligation`
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluateInput<'a> {
    /// All registered obligations, decoded from the L1
    /// `force_include_registry_address`. Keyed by obligation_id.
    pub obligations: &'a BTreeMap<ObligationId, ObligationSnapshot>,
    /// L2 tx hashes the sequencer has committed via
    /// `Intent::CommitL2StateRoot` since this daemon last ran.
    /// `BLAKE3(tx_bytes)` per
    /// `gsx_execution::force_include::tx_hash`.
    pub committed_batch_tx_hashes: &'a BTreeSet<TxHash>,
    /// Current L1 block height (from a recent L1 RPC poll).
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
///    is in `committed_batch_tx_hashes`.
/// 2. All `SlashMissedForceInclude` for `Pending` obligations
///    whose `deadline_l1_height < current_l1_height` AND were
///    NOT honored this tick.
/// 3. All `EjectSequencer` for `Slashed` obligations past the
///    ejection window AND not yet in `ejected_obligations`.
///
/// Honor-first ordering is load-bearing: if a tx lands inside
/// the deadline grace AND the deadline also passes in the same
/// tick, the daemon must emit `MarkHonored` rather than
/// `SlashMissedForceInclude`. The substrate would still reject
/// the slash (status is Pending → Honored transition), but
/// honoring keeps the sequencer's bond intact.
pub fn evaluate(input: EvaluateInput<'_>) -> Vec<DaemonAction> {
    let mut actions = Vec::new();
    let mut honored_this_tick: BTreeSet<ObligationId> = BTreeSet::new();

    // Pass 1: Pending obligations whose tx_hash was committed.
    for (id, ob) in input.obligations {
        if ob.status != ObligationStatus::Pending {
            continue;
        }
        if input.committed_batch_tx_hashes.contains(&ob.tx_hash) {
            actions.push(DaemonAction::MarkHonored { obligation_id: *id });
            honored_this_tick.insert(*id);
        }
    }

    // Pass 2: Pending obligations past deadline that were NOT
    // honored this tick.
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

    fn no_committed() -> BTreeSet<TxHash> {
        BTreeSet::new()
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
        let mut committed = BTreeSet::new();
        committed.insert([0x42; 32]);

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
    fn honor_wins_over_slash_in_same_tick() {
        // The obligation is BOTH committed AND past its deadline
        // (sequencer was late but still honored). Daemon must
        // emit MarkHonored, not Slash — the substrate rejects
        // the slash anyway (Pending → Honored transition), but
        // emitting honor preserves the sequencer's bond.
        let mut map = BTreeMap::new();
        map.insert(id(1), ob(0x42, 100, ObligationStatus::Pending));
        let mut committed = BTreeSet::new();
        committed.insert([0x42; 32]);

        let actions = evaluate(EvaluateInput {
            obligations: &map,
            committed_batch_tx_hashes: &committed,
            current_l1_height: 200,
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
        let mut committed = BTreeSet::new();
        committed.insert([0x42; 32]);

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
        // id(1): Pending, will be honored
        map.insert(id(1), ob(0x11, 100, ObligationStatus::Pending));
        // id(2): Pending, will be slashed
        map.insert(id(2), ob(0x22, 50, ObligationStatus::Pending));
        // id(3): Slashed past window, will be ejected
        map.insert(id(3), ob(0x33, 10, ObligationStatus::Slashed));
        // id(4): Slashed inside window, no-op (deadline well
        // above current_l1_height even with the +window)
        map.insert(id(4), ob(0x44, 50_000, ObligationStatus::Slashed));

        let mut committed = BTreeSet::new();
        committed.insert([0x11; 32]);

        // current_l1_height past id(2)'s deadline AND past
        // id(3)'s deadline+EJECTION_WINDOW, but still below
        // id(4)'s deadline.
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
        let mut committed = BTreeSet::new();
        committed.insert([3u8; 32]);
        committed.insert([6u8; 32]);

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
}
