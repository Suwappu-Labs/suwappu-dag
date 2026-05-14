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
    AuthorityMemberView, BlockView, EpochView, EventView, IntentView, StateView, SubmitIntentError,
    TransactionView, ValidatorMemberView,
};
use tokio::sync::{broadcast, mpsc::UnboundedSender};

use crate::{
    client::{verify_signed_intent, AuthOutcome},
    daemon::State,
    events::{Event, EventLog, Lane},
};

/// Translate a daemon `Event` into the JSON-safe `EventView`
/// projection. Pure function. Lane is rendered as the same lowercase
/// string the NDJSON file uses (matches `Event`'s
/// `#[serde(rename_all = "lowercase")]`).
fn event_to_view(ev: Event) -> EventView {
    let lane = match ev.lane {
        Lane::Main => "main",
        Lane::FastPath => "fastpath",
        Lane::Ltp => "ltp",
        Lane::Client => "client",
    }
    .to_string();
    EventView {
        t_ms: ev.t_ms,
        region: ev.region,
        lane,
        event: ev.event,
        round: ev.round,
        cert_hash: ev.cert_hash,
        tx_hash: ev.tx_hash,
        peer: ev.peer,
        intent_hashes: ev.intent_hashes,
        authority_id: ev.authority_id,
        kind: ev.kind,
        received_60s: ev.received_60s,
    }
}

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

/// Adapter wrapping the daemon's shared `Arc<State>` plus the
/// write-path machinery (`intent_tx` mpsc + `network_id`) needed for
/// `gsx_submitIntent`. The TCP/bincode client wire and this adapter
/// fan into the SAME sender, so the two ingress paths share order
/// guarantees and the round driver doesn't notice which wire an
/// intent arrived on.
///
/// T6: also holds a `broadcast::Sender<EventView>` that bridges the
/// gsx-node `Event` stream into the JSON-safe `EventView` shape the
/// RPC surface exposes. One bridge task converts events at the
/// adapter boundary; every WS subscriber gets its own `Receiver` and
/// drops it cleanly on disconnect.
pub struct NodeStateView {
    state: Arc<State>,
    intent_tx: UnboundedSender<Intent>,
    network_id: String,
    event_view_tx: broadcast::Sender<EventView>,
}

impl NodeStateView {
    pub(crate) fn new(
        state: Arc<State>,
        intent_tx: UnboundedSender<Intent>,
        network_id: String,
        log: &EventLog,
    ) -> Self {
        // T6: bridge gsx-node Event → gsx-rpc EventView once per
        // daemon (not per subscriber). 1024 slot ring buffer matches
        // EventLog's broadcast buffer.
        let (event_view_tx, _) = broadcast::channel::<EventView>(1024);
        let bridge_tx = event_view_tx.clone();
        let mut rx = log.subscribe();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(ev) => {
                        // No receivers? `send` errors; ignore — RPC
                        // server still runs even with zero subscribers.
                        let _ = bridge_tx.send(event_to_view(ev));
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // Bridge can't see the dropped events; the WS
                        // peer will see Lagged from its own receiver
                        // (the buffers are sized identically).
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::debug!("rpc adapter: event bridge channel closed");
                        return;
                    }
                }
            }
        });
        Self {
            state,
            intent_tx,
            network_id,
            event_view_tx,
        }
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

    async fn submit_intent(
        &self,
        intent_bincode: Vec<u8>,
        signature: Vec<u8>,
        signer_pubkey_hash: [u8; 32],
    ) -> Result<[u8; 32], SubmitIntentError> {
        // 1. Decode the bincode-serialized Intent. SDK clients build
        //    this exact form before signing, so we can reuse the same
        //    bytes for both the digest and the channel send.
        let intent: Intent = bincode::deserialize(&intent_bincode)
            .map_err(|e| SubmitIntentError::BadIntentEncoding(e.to_string()))?;

        // 2. Verify the signature using the same gate the TCP wire uses.
        match verify_signed_intent(
            &self.state,
            &self.network_id,
            &intent,
            &signature,
            &signer_pubkey_hash,
        )
        .await
        {
            AuthOutcome::Ok => {}
            AuthOutcome::UnknownSigner => return Err(SubmitIntentError::UnknownSigner),
            AuthOutcome::BadSignature => return Err(SubmitIntentError::BadSignature),
        }

        // 3. Compute the canonical intent hash — same blake3 over the
        //    bincode bytes the round driver will use when it indexes
        //    this intent into `tx_to_block`. Reusing the bytes (rather
        //    than re-serializing) guarantees the SDK's hash matches.
        let intent_hash: [u8; 32] = *blake3::hash(&intent_bincode).as_bytes();

        // 4. Enqueue. `UnboundedSender::send` only fails if the receiver
        //    has been dropped (round driver task died). Either way the
        //    caller should treat it as a transient failure to retry.
        self.intent_tx
            .send(intent)
            .map_err(|_| SubmitIntentError::EnqueueFull)?;

        Ok(intent_hash)
    }

    fn subscribe_events(&self) -> broadcast::Receiver<EventView> {
        // Per-subscriber receiver. The bridge task spawned in
        // `NodeStateView::new` keeps the buffer pumped; dropping
        // this receiver cancels the subscription cleanly.
        self.event_view_tx.subscribe()
    }
}
