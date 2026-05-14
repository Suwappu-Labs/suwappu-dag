//! Adapter that implements `gsx_rpc::StateView` against the running
//! daemon's `State`. This module is the only bridge between the daemon
//! and the JSON-RPC server — `gsx-rpc` itself has no dependency on
//! `gsx-node`, so the trait-and-adapter pattern keeps the dep graph
//! a DAG.
//!
//! Lock acquisition follows the canonical order documented on `State`:
//! `inner → dag → stake_table → authority_registry → validator_registry
//! → votes → blocks → committed`. The four methods here each take at
//! most ONE read lock, so the order is trivially respected — but every
//! method clones the result out under the guard and drops the guard
//! before returning, so an RPC client can't stall the consensus loop.

use std::sync::Arc;

use gsx_execution::Intent;
use gsx_rpc::context::{
    AuthorityMemberView, BlockView, EpochView, IntentView, StateView, TransactionView,
    ValidatorMemberView,
};

use crate::daemon::State;

/// Translate a daemon `Intent` into the JSON-safe `IntentView`
/// projection. Pure function; no state access. Address + stake +
/// proof_ref bytes are hex-encoded with `0x` prefix to match the rest
/// of the RPC surface (BalanceView, BlockView.cert_hash, etc).
fn intent_to_view(intent: &Intent) -> IntentView {
    match intent {
        Intent::Transfer { from, to, amount } => IntentView::Transfer {
            from: format!("0x{}", hex::encode(from)),
            to: format!("0x{}", hex::encode(to)),
            amount: amount.to_string(),
        },
        Intent::AdmitAuthority {
            authority_id,
            stake_gsx,
            mldsa_public_key,
            bls_public_key,
        } => IntentView::AdmitAuthority {
            authority_id: *authority_id,
            stake_gsx: stake_gsx.to_string(),
            mldsa_public_key_hex: hex::encode(mldsa_public_key),
            bls_public_key_hex: hex::encode(bls_public_key),
        },
        Intent::ExitAuthority { authority_id } => IntentView::ExitAuthority {
            authority_id: *authority_id,
        },
        Intent::EjectAuthority {
            authority_id,
            proof_ref,
        } => IntentView::EjectAuthority {
            authority_id: *authority_id,
            proof_ref: format!("0x{}", hex::encode(proof_ref)),
        },
    }
}

/// Read-only adapter wrapping the daemon's shared `Arc<State>`.
pub struct NodeStateView {
    state: Arc<State>,
}

impl NodeStateView {
    pub(crate) fn new(state: Arc<State>) -> Self {
        Self { state }
    }
}

impl StateView for NodeStateView {
    async fn epoch_snapshot(&self) -> EpochView {
        let inner = self.state.inner.lock().await;
        EpochView {
            current: inner.epoch.current,
            last_boundary_round: inner.epoch.last_boundary_round,
            rounds_per_epoch: inner.epoch.rounds_per_epoch,
        }
    }

    async fn authority_snapshot(&self) -> Vec<AuthorityMemberView> {
        let reg = self.state.authority_registry.read().await;
        reg.members()
            .map(|m| AuthorityMemberView {
                id: m.id,
                stake_gsx: m.stake_gsx,
                public_key_hex: hex::encode(&m.public_key_bytes),
            })
            .collect()
    }

    async fn validator_snapshot(&self) -> Vec<ValidatorMemberView> {
        let reg = self.state.validator_registry.read().await;
        reg.members()
            .map(|m| ValidatorMemberView {
                id: m.id,
                stake_gsx: m.stake_gsx.to_string(),
            })
            .collect()
    }

    async fn stake_for(&self, authority_id: u32) -> Option<u128> {
        let reg = self.state.authority_registry.read().await;
        reg.get(authority_id).map(|m| m.stake_gsx as u128)
    }

    async fn balance_for(&self, address: [u8; 20]) -> u128 {
        use gsx_execution::Substrate;
        let inner = self.state.inner.lock().await;
        inner.substrate.balance(&address)
    }

    async fn block_at_round(&self, round: u64) -> Option<BlockView> {
        // Resolve round → cert hash via the O(log n) index. Then look
        // up the block payload from state.blocks. Two locks, but no
        // guard held across .await — snapshot the cert hash, drop the
        // inner guard, then take the blocks lock (parking_lot, sync).
        let cert_hash = {
            let inner = self.state.inner.lock().await;
            inner.blocks_by_round.get(&round).copied()
        }?;
        let block_intents = {
            let blocks = self.state.blocks.lock();
            blocks.get(&cert_hash).map(|b| b.intents.clone())
        }?;
        Some(BlockView {
            round,
            cert_hash: format!("0x{}", hex::encode(cert_hash.0)),
            intents: block_intents.iter().map(intent_to_view).collect(),
        })
    }

    async fn transaction_by_hash(&self, tx_hash: [u8; 32]) -> Option<TransactionView> {
        // Resolve tx-hash → (round, cert, index) via the O(1) index,
        // then load the block and slice out the specific intent. Same
        // lock-discipline as block_at_round.
        let (round, cert_hash, index) = {
            let inner = self.state.inner.lock().await;
            inner.tx_to_block.get(&tx_hash).copied()
        }?;
        let intent = {
            let blocks = self.state.blocks.lock();
            blocks
                .get(&cert_hash)
                .and_then(|b| b.intents.get(index).cloned())
        }?;
        Some(TransactionView {
            tx_hash: format!("0x{}", hex::encode(tx_hash)),
            round,
            cert_hash: format!("0x{}", hex::encode(cert_hash.0)),
            index,
            intent: intent_to_view(&intent),
        })
    }
}
