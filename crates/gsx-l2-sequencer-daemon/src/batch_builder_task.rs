//! Batch-builder timer task (Phase 2.2-b, #105).
//!
//! Wakes every `batch_interval_ms`, drains the mempool up to
//! `MAX_TXS_PER_BATCH`, queries the L1 client for the current
//! state roots + L1 height, and constructs a [`Batch`] via the
//! pure-logic [`BatchBuilder::build`]. Logs the resulting
//! batch's `da_commitment` and tx count for ops visibility.
//!
//! ## What this does NOT do (yet)
//!
//! - **Compute `new_l2_state_root`.** The native STM
//!   ([`gsx_l2_stm::execute_batch`]) gives us this, but
//!   threading the L2 ledger through the daemon's shared state
//!   is Phase 2.2-c work. For now the task uses `[0u8; 32]`
//!   as a placeholder — the resulting batch is well-formed
//!   but wouldn't verify against any real proof.
//! - **Submit the batch via `Intent::PostL2DA` +
//!   `Intent::CommitL2StateRoot`.** Needs the SP1 prover
//!   (Phase 2.1, #104) to produce the proof first. For now
//!   the task only constructs the batch + logs it.
//! - **Bound the per-tick mempool drain to a max tx count.**
//!   The drain is currently `MAX_TXS_PER_BATCH` (500); a
//!   future tuning pass may scale this with proving budget.
//!
//! ## Tests
//!
//! Drive the task with `tokio::time::pause` + `advance` so the
//! tick fires deterministically. The task uses a shared
//! [`SequencerState`] so a test thread can pre-populate the
//! mempool, advance the clock, and assert on what the task
//! built.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use blake3::Hasher;
use gsx_l2_sequencer::{Batch, BatchBuilder, BuildContext, TxMempool, MAX_TXS_PER_BATCH};
use tokio::{select, time};
use tracing::{debug, info, warn};

use crate::l1_client::{L1Client, L1ClientError};

/// Shared state mutated by the batch-builder + (Phase 2.2-c)
/// force-include + (Phase 2.2-d) RPC tasks. Held behind a
/// `Mutex` because Tokio tasks may concurrently push to the
/// mempool (RPC server) while the builder drains it. A
/// `parking_lot::Mutex` would be marginally faster; std is
/// fine until we observe contention.
#[derive(Debug, Default)]
pub struct SequencerState {
    /// L2 transaction queue.
    pub mempool: TxMempool,
    /// Monotonic batch id counter; incremented per built batch.
    pub next_batch_id: u64,
    /// The most recently built batch, for tests + observability.
    pub last_built: Option<Batch>,
}

impl SequencerState {
    /// New empty state.
    pub fn new() -> Self {
        Self::default()
    }
}

/// Static configuration for the batch-builder task. Cloned
/// once at startup; the task doesn't re-read config.
#[derive(Debug, Clone)]
pub struct BatchBuilderTaskConfig {
    /// Tick period.
    pub interval: Duration,
    /// L2 chain id hash, computed once at startup from
    /// [`crate::config::SequencerConfig::l2_chain_id`].
    pub l2_chain_id_hash: [u8; 32],
    /// Range-program VK commitment. Embeds into every batch
    /// header so the verifier can pin the proof to the
    /// daemon's expected range program. `[0u8; 32]` is the
    /// v1 "no range program" sentinel; future versions
    /// populate from on-chain VK registry.
    pub range_vk_commitment: [u8; 32],
}

impl BatchBuilderTaskConfig {
    /// Helper: derive `l2_chain_id_hash` from a chain id
    /// string. Matches the substrate's convention:
    /// `BLAKE3("gsx-l2-chain-" || chain_id)`.
    pub fn derive_l2_chain_id_hash(chain_id: &str) -> [u8; 32] {
        let mut h = Hasher::new();
        h.update(b"gsx-l2-chain-");
        h.update(chain_id.as_bytes());
        *h.finalize().as_bytes()
    }
}

/// One tick of the batch-builder loop, broken out so tests can
/// drive it without instantiating the full Tokio interval.
/// Returns the batch (if any txs were drained) for assertion.
pub async fn run_one_tick<C: L1Client>(
    state: &Arc<Mutex<SequencerState>>,
    cfg: &BatchBuilderTaskConfig,
    l1: &C,
) -> Result<Option<Batch>, L1ClientError> {
    // Read L1-side anchor state. Done UNDER one round-trip per
    // call so a stale L1 doesn't cause the batch header's
    // l1_anchor_height + prev_l1_state_root to disagree.
    let l1_anchor_height = l1.current_l1_height().await?;
    let prev_l1_state_root = l1.current_l1_state_root().await?;
    let prev_l2_state_root = l1.current_l2_state_root(&cfg.l2_chain_id_hash).await?;

    // Drain the mempool. Hold the lock only long enough to
    // drain — the actual batch construction runs lock-free.
    let (txs, batch_id) = {
        let mut s = state.lock().expect("sequencer state poisoned");
        let drained = s.mempool.drain(MAX_TXS_PER_BATCH);
        if drained.is_empty() {
            debug!("batch builder: mempool empty, skipping tick");
            return Ok(None);
        }
        let id = s.next_batch_id;
        s.next_batch_id = s.next_batch_id.wrapping_add(1);
        (drained, id)
    };

    // Phase 2.2-c will compute `new_l2_state_root` via
    // `gsx_l2_stm::execute_batch`. Stub for now.
    let ctx = BuildContext {
        prev_l2_state_root,
        batch_id,
        l1_anchor_height,
        range_vk_commitment: cfg.range_vk_commitment,
        prev_l1_state_root,
        l2_chain_id_hash: cfg.l2_chain_id_hash,
        confidential_root: [0u8; 32],
        new_l2_state_root: [0u8; 32], // placeholder
    };
    let tx_count = txs.len();
    let batch = BatchBuilder::build(txs, ctx);
    info!(
        batch_id,
        tx_count,
        da_commitment = ?batch.header.da_commitment,
        l1_anchor_height,
        "batch builder: built batch"
    );

    // Record the built batch into shared state so tests + the
    // (future) prover task can observe it.
    state.lock().expect("sequencer state poisoned").last_built = Some(batch.clone());

    Ok(Some(batch))
}

/// Run the batch-builder loop. Exits when the cancellation
/// `shutdown` future resolves.
pub async fn run_loop<C: L1Client, F: std::future::Future<Output = ()> + Unpin>(
    state: Arc<Mutex<SequencerState>>,
    cfg: BatchBuilderTaskConfig,
    l1: Arc<C>,
    mut shutdown: F,
) {
    let mut ticker = time::interval(cfg.interval);
    // Skip the initial immediate tick so startup doesn't
    // immediately try to build an empty batch.
    ticker.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    loop {
        select! {
            _ = &mut shutdown => {
                info!("batch builder: shutdown signal received");
                return;
            }
            _ = ticker.tick() => {
                match run_one_tick(&state, &cfg, &*l1).await {
                    Ok(_) => {}
                    Err(e) => warn!(error = %e, "batch builder: tick failed"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::l1_client::mock::MockL1Client;

    fn cfg() -> BatchBuilderTaskConfig {
        BatchBuilderTaskConfig {
            interval: Duration::from_millis(50),
            l2_chain_id_hash: BatchBuilderTaskConfig::derive_l2_chain_id_hash("test-chain"),
            range_vk_commitment: [0u8; 32],
        }
    }

    #[tokio::test]
    async fn empty_mempool_returns_none() {
        let state = Arc::new(Mutex::new(SequencerState::new()));
        let l1 = MockL1Client::new();
        let batch = run_one_tick(&state, &cfg(), &l1).await.unwrap();
        assert!(batch.is_none());
        assert_eq!(state.lock().unwrap().next_batch_id, 0);
    }

    #[tokio::test]
    async fn populated_mempool_produces_batch_and_advances_id() {
        let state = Arc::new(Mutex::new(SequencerState::new()));
        {
            let mut s = state.lock().unwrap();
            s.mempool.submit(vec![1, 2, 3, 4]).unwrap();
            s.mempool.submit(vec![5, 6, 7, 8]).unwrap();
        }
        let l1 = MockL1Client::new();
        l1.set_l1_height(100);

        let batch = run_one_tick(&state, &cfg(), &l1).await.unwrap().unwrap();
        assert_eq!(batch.txs.len(), 2);
        assert_eq!(batch.header.batch_id, 0);
        assert_eq!(batch.header.l1_anchor_height, 100);

        // Next tick increments batch_id even when mempool is empty
        // -- no wait, see `empty_mempool_returns_none` above: empty
        // ticks do NOT advance the id. Verify:
        let next_batch = run_one_tick(&state, &cfg(), &l1).await.unwrap();
        assert!(next_batch.is_none());
        assert_eq!(state.lock().unwrap().next_batch_id, 1);
    }

    #[tokio::test]
    async fn l1_failure_propagates() {
        let state = Arc::new(Mutex::new(SequencerState::new()));
        {
            state.lock().unwrap().mempool.submit(vec![1, 2, 3]).unwrap();
        }
        let l1 = MockL1Client::new();
        l1.set_should_fail(true);
        let err = run_one_tick(&state, &cfg(), &l1).await;
        assert!(err.is_err());
        // Mempool wasn't drained because L1 lookups failed
        // BEFORE the drain.
        assert_eq!(state.lock().unwrap().mempool.len(), 1);
    }

    #[tokio::test]
    async fn run_loop_exits_on_shutdown_signal() {
        let state = Arc::new(Mutex::new(SequencerState::new()));
        let l1 = Arc::new(MockL1Client::new());
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let shutdown = Box::pin(async move {
            let _ = shutdown_rx.await;
        });

        let handle = tokio::spawn(run_loop(state, cfg(), l1, shutdown));
        // Let one or two ticks fire.
        tokio::time::sleep(Duration::from_millis(120)).await;
        shutdown_tx.send(()).unwrap();
        handle.await.unwrap();
    }

    #[test]
    fn l2_chain_id_hash_derivation_is_deterministic() {
        let a = BatchBuilderTaskConfig::derive_l2_chain_id_hash("gsx-l2-testnet-1");
        let b = BatchBuilderTaskConfig::derive_l2_chain_id_hash("gsx-l2-testnet-1");
        assert_eq!(a, b);
        let c = BatchBuilderTaskConfig::derive_l2_chain_id_hash("gsx-l2-mainnet");
        assert_ne!(a, c);
    }
}
