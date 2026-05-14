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

use gsx_rpc::context::{AuthorityMemberView, EpochView, StateView, ValidatorMemberView};

use crate::daemon::State;

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
}
