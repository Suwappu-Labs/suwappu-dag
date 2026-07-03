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
    substrate::{FeeCharge, Intent, Substrate},
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
    /// Mysticeti round at which this block was committed.
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
    /// Number of intents skipped after the first error.
    pub skipped: usize,
    /// State root after the block.
    pub post_root: [u8; 32],
}

/// Apply `block` to `substrate`. Returns an [`ExecutionReport`].
///
/// Fee-less path: every intent applies with no fee charged, exactly as
/// before FEE-1. Equivalent to [`execute_block_with_fees`] with an empty
/// fee slice.
pub fn execute_block<S: Substrate>(substrate: &mut S, block: &Block) -> ExecutionReport {
    execute_block_with_fees(substrate, block, &[])
}

/// Apply `block` to `substrate`, settling a per-intent fee where one is
/// present (FEE-1 Phase 1 / IQ-007 Option A).
///
/// `fees` is positionally aligned to `block.intents`: `fees[i]` is the
/// fee for `block.intents[i]`. A `None` entry (or an index past the end
/// of `fees`) means "no sponsor" — that intent applies fee-free, exactly
/// as today. A `Some(FeeCharge)` charges the flat `max_fee` from the
/// payer to the authority-rewards-pool sink **atomically with** the
/// intent (see [`Substrate::apply_intent_with_fee`]): the intent + fee is
/// one all-or-nothing unit, so a fee failure reverts the intent and vice
/// versa. This settles fees at execution only — it does not touch the
/// joint-quorum commit path or the checkpoint surface (IQ-007 keeps the
/// two rings independent of payment).
///
/// Determinism: fees are consumed in intent order via the positional
/// slice; no map iteration is involved.
pub fn execute_block_with_fees<S: Substrate>(
    substrate: &mut S,
    block: &Block,
    fees: &[Option<FeeCharge>],
) -> ExecutionReport {
    // Length-alignment guard: the empty slice is the fee-less path
    // (every intent applies fee-free). A NON-empty `fees` must be
    // positionally aligned to `block.intents` — a misaligned envelope
    // would silently make trailing intents fee-free, so fail loudly on
    // the internal-invariant violation instead.
    assert!(
        fees.is_empty() || fees.len() == block.intents.len(),
        "execute_block_with_fees: fees length {} must be empty or match intents length {}",
        fees.len(),
        block.intents.len(),
    );

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
        let fee = fees.get(idx).and_then(|f| f.as_ref());
        match substrate.apply_intent_with_fee(intent, fee) {
            Ok(()) => applied += 1,
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
    fn execute_block_with_fees_charges_only_sponsored_intents() {
        use crate::reserved::authority_rewards_pool_address;
        let sponsor = addr(9);
        let sink = authority_rewards_pool_address();
        let mut s = InMemorySubstrate::from_balances([(addr(1), 100), (sponsor, 50)]);
        let block = Block {
            round: 1,
            intents: vec![
                Intent::Transfer {
                    from: addr(1),
                    to: addr(2),
                    amount: 10,
                },
                Intent::Transfer {
                    from: addr(1),
                    to: addr(3),
                    amount: 10,
                },
            ],
        };
        // Only the first intent is sponsored (max_fee = 5); the second
        // carries no fee (None).
        let fees = vec![
            Some(crate::substrate::FeeCharge {
                payer: sponsor,
                max_fee: 5,
            }),
            None,
        ];
        let report = execute_block_with_fees(&mut s, &block, &fees);
        assert_eq!(report.applied, 2);
        assert!(report.first_error.is_none());
        // Both transfers landed.
        assert_eq!(s.balance(&addr(2)), 10);
        assert_eq!(s.balance(&addr(3)), 10);
        // Exactly one fee of 5 was collected (from the sponsored intent).
        assert_eq!(s.balance(&sink), 5);
        assert_eq!(s.balance(&sponsor), 45);
    }

    #[test]
    #[should_panic(expected = "must be empty or match intents length")]
    fn execute_block_with_misaligned_fees_panics() {
        let mut s = InMemorySubstrate::from_balances([(addr(1), 100)]);
        let block = Block {
            round: 0,
            intents: vec![
                Intent::Transfer {
                    from: addr(1),
                    to: addr(2),
                    amount: 1,
                },
                Intent::Transfer {
                    from: addr(1),
                    to: addr(3),
                    amount: 1,
                },
            ],
        };
        // One fee entry for a two-intent block — misaligned, must panic
        // loudly rather than silently leave intent[1] fee-free.
        let fees = vec![Some(crate::substrate::FeeCharge {
            payer: addr(9),
            max_fee: 1,
        })];
        let _ = execute_block_with_fees(&mut s, &block, &fees);
    }

    #[test]
    fn execute_block_with_empty_fees_matches_execute_block() {
        let intents = vec![Intent::Transfer {
            from: addr(1),
            to: addr(2),
            amount: 10,
        }];
        let mut a = InMemorySubstrate::from_balances([(addr(1), 100)]);
        let mut b = InMemorySubstrate::from_balances([(addr(1), 100)]);
        let block = Block { round: 0, intents };
        let ra = execute_block(&mut a, &block);
        let rb = execute_block_with_fees(&mut b, &block, &[]);
        assert_eq!(ra.post_root, rb.post_root);
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
