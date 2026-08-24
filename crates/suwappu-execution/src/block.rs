//! Block executor — applies a linearized intent sequence through a
//! [`Substrate`].
//!
//! In production this consumes the output of
//! `suwappu_consensus::finalize` (DAG-S4) → the linearized certificate
//! sequence → the intents carried by those certificates → the substrate
//! adapter. Phase-1 lifts the cert-to-intent mapping out of scope and
//! takes the intent list directly; the cert layer is provided by
//! `suwappu-consensus` and is consumed by this adapter once the suwappu-db tag
//! is cut.

use serde::{Deserialize, Serialize};
use suwappu_consensus::Round;

use crate::{
    error::ExecutionError,
    substrate::{Intent, Substrate},
};

/// A block of intents to apply atomically against a [`Substrate`].
///
/// The block executor applies intents in order; if any intent fails,
/// every later intent in the block is skipped (Phase-1 stop-on-error).
/// The suwappu-db bundle abstraction (paper §7.3) provides full atomic
/// rollback on cross-VM writes; that lands with the real substrate
/// wire-up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    /// DagBft round at which this block was committed.
    pub round: Round,
    /// Intents to apply. Order is the linearized order produced by
    /// `suwappu_consensus::finalize`.
    pub intents: Vec<Intent>,
}

/// Per-block execution report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionReport {
    /// Round of the block.
    pub round: Round,
    /// Number of intents applied successfully.
    pub applied: usize,
    /// First error encountered, if any. Phase-1 stops on first error;
    /// later intents are reported as `skipped`.
    pub first_error: Option<(usize, ExecutionError)>,
    /// Number of intents skipped: everything after the first
    /// error, plus benign no-op replays (an already-claimed
    /// `TgeClaim`), which do not halt the block.
    pub skipped: usize,
    /// State root after the block.
    pub post_root: [u8; 32],
}

/// Apply `block` to `substrate`. Returns an [`ExecutionReport`].
pub fn execute_block<S: Substrate>(substrate: &mut S, block: &Block) -> ExecutionReport {
    let mut applied = 0;
    let mut first_error = None;
    let mut skipped = 0;

    // Plumb the current round through to the substrate so
    // intent arms that need a height (e.g., the exit-cooldown
    // gate on Withdraw*) can read it via
    // `current_block_height`. Adapters that don't override the
    // trait default ignore this.
    substrate.set_current_block_height(block.round);

    for (idx, intent) in block.intents.iter().enumerate() {
        if first_error.is_some() {
            skipped += 1;
            continue;
        }
        match substrate.apply_intent(intent) {
            Ok(()) => applied += 1,
            // A replayed TgeClaim is the EXPECTED outcome of wide
            // broadcast + per-node content-hash mempool dedup (two
            // proposers can include the same claim). The
            // idempotency guard already made it a no-op — treat it
            // as a benign skip instead of letting one duplicate
            // halt every intent behind it in the block.
            Err(ExecutionError::TgeAlreadyClaimed { .. }) => skipped += 1,
            Err(e) => first_error = Some((idx, e)),
        }
    }

    ExecutionReport {
        round: block.round,
        applied,
        first_error,
        skipped,
        post_root: substrate.state_root(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::substrate::{Address, InMemorySubstrate};

    fn addr(seed: u8) -> Address {
        [seed; 20]
    }

    #[test]
    fn empty_block_is_a_noop() {
        let mut s = InMemorySubstrate::from_balances([(addr(1), 100)]);
        let before_root = s.state_root();
        let report = execute_block(
            &mut s,
            &Block {
                round: 0,
                intents: vec![],
            },
        );
        assert_eq!(report.applied, 0);
        assert!(report.first_error.is_none());
        assert_eq!(report.post_root, before_root);
    }

    #[test]
    fn stops_on_first_error() {
        let mut s = InMemorySubstrate::from_balances([(addr(1), 100)]);
        let report = execute_block(
            &mut s,
            &Block {
                round: 1,
                intents: vec![
                    Intent::Transfer {
                        from: addr(1),
                        to: addr(2),
                        amount: 30,
                    },
                    Intent::Transfer {
                        from: addr(1),
                        to: addr(3),
                        amount: 1_000, // insufficient
                    },
                    Intent::Transfer {
                        from: addr(1),
                        to: addr(4),
                        amount: 10, // skipped
                    },
                ],
            },
        );
        assert_eq!(report.applied, 1);
        assert!(matches!(
            report.first_error,
            Some((1, ExecutionError::InsufficientBalance { .. }))
        ));
        assert_eq!(report.skipped, 1);
        assert_eq!(s.balance(&addr(2)), 30);
        assert_eq!(s.balance(&addr(4)), 0);
    }

    #[test]
    fn replay_reproduces_state_root() {
        let intents = vec![
            Intent::Transfer {
                from: addr(1),
                to: addr(2),
                amount: 10,
            },
            Intent::Transfer {
                from: addr(2),
                to: addr(3),
                amount: 5,
            },
        ];
        let mut s1 = InMemorySubstrate::from_balances([(addr(1), 100)]);
        let mut s2 = InMemorySubstrate::from_balances([(addr(1), 100)]);
        let block = Block { round: 0, intents };
        let r1 = execute_block(&mut s1, &block);
        let r2 = execute_block(&mut s2, &block);
        assert_eq!(r1.post_root, r2.post_root);
    }
}
