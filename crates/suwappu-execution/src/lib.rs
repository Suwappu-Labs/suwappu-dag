//! suwappu-execution — DAG block executor.
//!
//! Wires the [`suwappu-db`](https://github.com/Suwappu-Labs/suwappu-db)
//! execution substrate into the consensus pipeline. Implements the dispatch
//! from a Mysticeti-linearized block of intents through the OCC scheduler,
//! the state tree commit, and the outbound anchor pipeline.
//!
//! Paper mapping (§7):
//!
//! - §7.1 co-resident dual VM — delegated to `suwappu-db::vm`
//! - §7.2 polymorphic balance map — delegated to `suwappu-db::balance_slot`
//! - §7.3 checkpoint-synchronized cross-VM writes — delegated to `suwappu-db::bundle`
//! - §7.4 substrate integration — implemented in this crate
//!
//! Sprint scope:
//!
//! - DAG-S10: `Substrate` trait + in-memory adapter + block executor ✅
//! - DAG-S11: checkpoint cadence + Authority-Ring joint co-signature over
//!   the joint state commitment (Σ_EVM, Σ_Move)
//!
//! Load-bearing invariants inherited from suwappu-db:
//!
//! 1. Lane separation — only the bridge crate may mutate state.
//! 2. Dual-projection — EVM balanceOf == Move Coin::value for every address.
//! 3. Schedule determinism — block execution is independent of thread schedule.
//! 4. Bundle atomicity — cross-VM writes commit or revert as one.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod asset_registry;
pub mod authority_registry;
pub mod block;
pub mod burn_nullifier;
pub mod checkpoint;
pub mod delegation_registry;
pub mod eject_registry;
pub mod equivocation_registry;
pub mod error;
pub mod force_include;
pub mod l2_state;
pub mod reserved;
pub mod substrate;
pub mod suwappu_db_substrate;
pub mod validator_registry;

pub use block::{execute_block, Block, ExecutionReport};
pub use checkpoint::{
    ratify_checkpoint, sign_checkpoint, Checkpoint, CheckpointError, CheckpointHeight,
    CheckpointSignature, Checkpointer, CoSignedCheckpoint,
};
pub use error::ExecutionError;
pub use substrate::{Address, Balance, InMemorySubstrate, Intent, RewardsRing, Substrate};
pub use suwappu_db_substrate::SuwappuDbSubstrate;

/// Checkpoint cadence C — the rate at which the Authority Ring co-signs a
/// (Σ_EVM, Σ_Move) snapshot. Configured per testnet/mainnet; default below
/// targets the paper's 500 ms reliable-broadcast budget.
pub const DEFAULT_CHECKPOINT_CADENCE_ROUNDS: u32 = 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_cadence_default_is_one_round() {
        assert_eq!(DEFAULT_CHECKPOINT_CADENCE_ROUNDS, 1);
    }
}
