//! gsx-l2-bridge — L1↔L2 bridge off-chain validation library
//! (Track G G3.2, issue #101).
//!
//! ## Split of responsibility
//!
//! - **Substrate-side accounting** (debit user / credit escrow
//!   on `L1Lock`; debit escrow / credit recipient on
//!   `L2BurnProven`) lives in `gsx-execution::substrate` and
//!   uses the existing `credit_unchecked` helper + the
//!   reserved-address `bridge_escrow_address`. This split
//!   keeps the cryptographic + state-mutation surface in one
//!   place (substrate) and avoids a circular crate dep.
//! - **Off-chain validation** (this crate): payload sanity
//!   checks the sequencer, prover, bridge UI, or any other
//!   off-chain consumer should run BEFORE submitting an
//!   Intent. Catches obviously-malformed payloads at the
//!   submission boundary instead of after the consensus round.
//!
//! ## Phase split
//!
//! - **Phase 1 (this PR / #101)** — payload validation for
//!   `L1Lock` (deposit) + `L2BurnProven` (withdrawal); zero
//!   `amount` is rejected; max-amount sanity floor is
//!   enforced; `merkle_path` is validated for byte-shape only.
//! - **Phase 2** — full Merkle proof verification once the
//!   L2 state-root storage lands (depends on G2.2 phase 2).
//!   The validation surface stays stable; only the proof body
//!   check moves from "byte-shape only" to "Merkle inclusion
//!   against the proven L2 state".
//! - **Phase 3** — `L2ForceInclude` payload validation
//!   (Track G G3.4 #103 force-include slashing test).
//!
//! ## Bridge accounting invariant
//!
//! The substrate-side enforcement is the load-bearing
//! guarantee:
//!
//! > At every L1 block boundary,
//! >   `balance(bridge_escrow_address)
//! >     == sum_of_unwithdrawn_L2_deposits`.
//!
//! Off-chain validators (this crate) cannot enforce this
//! invariant directly — they don't see the chain state. But
//! they CAN catch local-payload bugs (zero amount, malformed
//! recipient, missing merkle_path) before they hit the
//! mempool, reducing wasted block-space + RPC noise.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 20-byte EVM-compatible address. Mirrors
/// `gsx_execution::substrate::Address` without taking a
/// crate dep (avoids the circularity in the G3.2 split).
pub type Address = [u8; 20];

/// Balance type. Mirrors `gsx_execution::substrate::Balance`.
pub type Balance = u128;

/// Maximum reasonable bridge payload size in bytes for the
/// `L2BurnProven::merkle_path` field. The phase-2 merkle-path
/// encoding is bounded at log2(L2_NULLIFIER_TREE_DEPTH) × 32
/// = 32 levels × 32 B/digest = 1024 B max. We allow 4× slack
/// for any envelope overhead.
pub const MAX_MERKLE_PATH_BYTES: usize = 4096;

/// Minimum bridge deposit amount in GSX (smallest unit).
/// Below this is dust — the L1 gas cost of locking + the L2
/// gas cost of crediting exceed the deposit value. The
/// substrate accepts dust deposits (no on-chain rule against
/// them) but the off-chain UI should warn the user.
pub const DUST_THRESHOLD_GSX: Balance = 100; // 100 base units

/// Errors returned by off-chain bridge-payload validation.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum BridgeError {
    /// `amount` was zero. The substrate accepts zero-amount
    /// Intents as no-ops, but the off-chain UI should reject
    /// them as user error (the user paid gas for nothing).
    #[error("bridge amount must be > 0")]
    ZeroAmount,

    /// `amount` was below the dust threshold. Soft warning;
    /// callers may choose to override.
    #[error("bridge amount {amount} below dust threshold {threshold}")]
    BelowDustThreshold {
        /// Amount the caller tried to bridge.
        amount: Balance,
        /// Configured dust threshold.
        threshold: Balance,
    },

    /// `merkle_path` was empty.
    #[error("merkle_path must not be empty")]
    EmptyMerklePath,

    /// `merkle_path` exceeded the maximum reasonable size.
    /// Bound at 4 KiB; full L2 nullifier-tree paths fit in
    /// well under 1 KiB.
    #[error("merkle_path exceeds maximum {max} bytes: got {got}")]
    MerklePathTooLong {
        /// Configured maximum.
        max: usize,
        /// Observed length.
        got: usize,
    },

    /// `merkle_path` length was not a multiple of 32 bytes
    /// (each level of the path is one 32-byte digest).
    /// Phase-1 byte-shape check; phase-2 will additionally
    /// validate the path proves inclusion under the
    /// committed L2 state root.
    #[error("merkle_path length {got} is not a multiple of 32 bytes")]
    MerklePathAlignment {
        /// Observed length.
        got: usize,
    },
}

/// L1→L2 deposit payload. Validates the parameters a caller
/// is about to submit as `Intent::L1Lock`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct L1LockPayload {
    /// L1 address being debited.
    pub user_address: Address,
    /// L2 address being credited (may differ from
    /// `user_address`; some bridge UIs let the user specify
    /// a different L2 recipient).
    pub l2_recipient: Address,
    /// Amount being bridged (GSX base units).
    pub amount: Balance,
}

impl L1LockPayload {
    /// Validate the payload's shape. Returns `Err` on
    /// strict (always-rejected) issues + `Ok(Some(warning))`
    /// on soft warnings (dust threshold). Callers should
    /// surface soft warnings to the user before submitting.
    pub fn validate(&self) -> Result<Option<BridgeError>, BridgeError> {
        if self.amount == 0 {
            return Err(BridgeError::ZeroAmount);
        }
        if self.amount < DUST_THRESHOLD_GSX {
            return Ok(Some(BridgeError::BelowDustThreshold {
                amount: self.amount,
                threshold: DUST_THRESHOLD_GSX,
            }));
        }
        Ok(None)
    }
}

/// L2→L1 withdrawal payload. Validates the parameters a
/// caller is about to submit as `Intent::L2BurnProven`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct L2BurnProvenPayload {
    /// L2 batch id whose committed state root proves the burn.
    pub batch_id: u64,
    /// L1 address receiving the unlocked balance.
    pub recipient: Address,
    /// Amount being unlocked (GSX base units).
    pub amount: Balance,
    /// Merkle proof binding the burn to the proven L2 state.
    /// Phase 1: byte-shape check only. Phase 2: full
    /// inclusion verification against the L2 state root
    /// stored at `l2_registry_address`.
    pub merkle_path: Vec<u8>,
}

impl L2BurnProvenPayload {
    /// Validate the payload's shape. Phase-1 byte-shape
    /// checks; phase-2 will additionally invoke the Merkle
    /// proof verifier once the L2 state-root storage lands
    /// (G2.2 phase 2).
    pub fn validate(&self) -> Result<(), BridgeError> {
        if self.amount == 0 {
            return Err(BridgeError::ZeroAmount);
        }
        if self.merkle_path.is_empty() {
            return Err(BridgeError::EmptyMerklePath);
        }
        if self.merkle_path.len() > MAX_MERKLE_PATH_BYTES {
            return Err(BridgeError::MerklePathTooLong {
                max: MAX_MERKLE_PATH_BYTES,
                got: self.merkle_path.len(),
            });
        }
        if self.merkle_path.len() % 32 != 0 {
            return Err(BridgeError::MerklePathAlignment {
                got: self.merkle_path.len(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(b: u8) -> Address {
        [b; 20]
    }

    // ----- L1LockPayload -----

    #[test]
    fn l1_lock_valid_passes() {
        let p = L1LockPayload {
            user_address: addr(1),
            l2_recipient: addr(2),
            amount: 1_000_000,
        };
        assert_eq!(p.validate(), Ok(None));
    }

    #[test]
    fn l1_lock_zero_amount_rejected() {
        let p = L1LockPayload {
            user_address: addr(1),
            l2_recipient: addr(2),
            amount: 0,
        };
        assert_eq!(p.validate(), Err(BridgeError::ZeroAmount));
    }

    #[test]
    fn l1_lock_dust_returns_soft_warning() {
        let p = L1LockPayload {
            user_address: addr(1),
            l2_recipient: addr(2),
            amount: DUST_THRESHOLD_GSX - 1,
        };
        let result = p.validate();
        assert!(matches!(
            result,
            Ok(Some(BridgeError::BelowDustThreshold { .. }))
        ));
    }

    #[test]
    fn l1_lock_at_dust_threshold_passes_clean() {
        let p = L1LockPayload {
            user_address: addr(1),
            l2_recipient: addr(2),
            amount: DUST_THRESHOLD_GSX,
        };
        assert_eq!(p.validate(), Ok(None));
    }

    // ----- L2BurnProvenPayload -----

    #[test]
    fn l2_burn_valid_passes() {
        let p = L2BurnProvenPayload {
            batch_id: 7,
            recipient: addr(3),
            amount: 1_000_000,
            merkle_path: vec![0u8; 320], // 10 levels × 32 B
        };
        assert!(p.validate().is_ok());
    }

    #[test]
    fn l2_burn_zero_amount_rejected() {
        let p = L2BurnProvenPayload {
            batch_id: 7,
            recipient: addr(3),
            amount: 0,
            merkle_path: vec![0u8; 320],
        };
        assert_eq!(p.validate(), Err(BridgeError::ZeroAmount));
    }

    #[test]
    fn l2_burn_empty_merkle_path_rejected() {
        let p = L2BurnProvenPayload {
            batch_id: 7,
            recipient: addr(3),
            amount: 1_000_000,
            merkle_path: vec![],
        };
        assert_eq!(p.validate(), Err(BridgeError::EmptyMerklePath));
    }

    #[test]
    fn l2_burn_oversized_merkle_path_rejected() {
        let p = L2BurnProvenPayload {
            batch_id: 7,
            recipient: addr(3),
            amount: 1_000_000,
            merkle_path: vec![0u8; MAX_MERKLE_PATH_BYTES + 32],
        };
        assert!(matches!(
            p.validate(),
            Err(BridgeError::MerklePathTooLong { .. })
        ));
    }

    #[test]
    fn l2_burn_misaligned_merkle_path_rejected() {
        let p = L2BurnProvenPayload {
            batch_id: 7,
            recipient: addr(3),
            amount: 1_000_000,
            merkle_path: vec![0u8; 333], // not a multiple of 32
        };
        assert!(matches!(
            p.validate(),
            Err(BridgeError::MerklePathAlignment { got: 333 })
        ));
    }

    #[test]
    fn l2_burn_at_max_merkle_path_passes() {
        let p = L2BurnProvenPayload {
            batch_id: 7,
            recipient: addr(3),
            amount: 1_000_000,
            merkle_path: vec![0u8; MAX_MERKLE_PATH_BYTES],
        };
        assert!(p.validate().is_ok());
    }

    #[test]
    fn l2_burn_minimum_merkle_path_passes() {
        // Single level (32-byte path) is the minimum.
        let p = L2BurnProvenPayload {
            batch_id: 7,
            recipient: addr(3),
            amount: 1_000_000,
            merkle_path: vec![0u8; 32],
        };
        assert!(p.validate().is_ok());
    }
}
