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
    /// Durable record of committed force-include tx hashes
    /// (#256). Replayed from disk at startup so force-include
    /// honor evidence survives a daemon restart and a later
    /// tick does not false-slash an already-honored obligation.
    /// The default is an in-memory-only store; `main` replaces
    /// it with a disk-backed one loaded from the data dir.
    pub committed_history: crate::committed_history::CommittedHistory,
}

impl SequencerState {
    /// New empty state with an in-memory-only committed-history
    /// store. The daemon binary swaps in a disk-backed store at
    /// startup via [`SequencerState::with_committed_history`].
    pub fn new() -> Self {
        Self::default()
    }

    /// New state seeded with a (typically disk-backed)
    /// committed-history store.
    pub fn with_committed_history(
        committed_history: crate::committed_history::CommittedHistory,
    ) -> Self {
        Self {
            committed_history,
            ..Self::default()
        }
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

/// Max number of times the atomic L1-snapshot loop re-reads the
/// height before giving up. The window for a height change
/// between two cheap RPCs is tiny; 3 attempts is generous.
const L1_SNAPSHOT_MAX_RETRIES: usize = 3;

/// Fetch a consistent `(l1_anchor_height, prev_l1_state_root)`
/// pair from the L1 (#248).
///
/// The [`L1Client`] trait exposes height and state-root as two
/// separate RPCs with no combined call, so a naive
/// `height` then `root` read can bind a `(height, root)` pair
/// that never coexisted if the L1 advances between the two
/// calls. We close that race with a refetch-on-mismatch loop:
/// read height, read root, then re-read height — if the height
/// is unchanged the root is consistent with it; otherwise retry
/// with the fresh height. After [`L1_SNAPSHOT_MAX_RETRIES`]
/// unstable reads we bail rather than bind a torn snapshot.
///
/// `current_l2_state_root` is intentionally NOT part of this
/// loop: #248 is about the `(height, l1_state_root)` pair only.
///
/// # Consistency guarantee (and its limits)
///
/// This loop guarantees the returned `(l1_height,
/// l1_state_root)` pair is consistent ONLY against a
/// *monotonic forward advance* of the L1 (height strictly
/// increasing between reads). It is NOT safe against an
/// equal-height reorg: if the L1 forks at height `100`,
/// abandons it, and re-forms a *different* chain that is also
/// at height `100` with a different state root, both height
/// reads observe `100`, the `height_after == height` check
/// passes, and the loop binds the post-reorg root to the
/// pre-reorg height (or vice versa) without detecting the
/// swap. Closing that gap needs a height+root-in-one-call RPC
/// or a block-hash check, neither of which the [`L1Client`]
/// trait exposes today.
///
/// Additionally, the L2 state root (`current_l2_state_root`,
/// read by the caller AFTER this snapshot returns) is OUTSIDE
/// this consistency window: it is fetched in a separate RPC
/// once the `(height, root)` pair is already bound, so an L1
/// advance between this snapshot and that read is not covered
/// here. Treat the returned pair as forward-advance-consistent
/// only — not as a full reorg-safe or cross-root-atomic
/// snapshot.
async fn fetch_consistent_l1_snapshot<C: L1Client>(
    l1: &C,
) -> Result<(u64, [u8; 32]), L1ClientError> {
    let mut height = l1.current_l1_height().await?;
    for _ in 0..L1_SNAPSHOT_MAX_RETRIES {
        let root = l1.current_l1_state_root().await?;
        let height_after = l1.current_l1_height().await?;
        if height_after == height {
            return Ok((height, root));
        }
        // L1 advanced between the two reads; the root we just
        // got may belong to `height_after`, not `height`. Retry
        // with the fresh height.
        height = height_after;
    }
    Err(L1ClientError::Rpc(format!(
        "l1 height unstable after {L1_SNAPSHOT_MAX_RETRIES} retries; cannot bind atomic (height, state_root) snapshot"
    )))
}

/// One tick of the batch-builder loop, broken out so tests can
/// drive it without instantiating the full Tokio interval.
/// Returns the batch (if any txs were drained) for assertion.
pub async fn run_one_tick<C: L1Client>(
    state: &Arc<Mutex<SequencerState>>,
    cfg: &BatchBuilderTaskConfig,
    l1: &C,
) -> Result<Option<Batch>, L1ClientError> {
    // #249: skip-idle-RPC. Peek the mempool FIRST and bail
    // before any L1 round-trip when there's nothing to batch.
    // This task is the only drainer and the RPC server only
    // *adds* to the mempool, so peek-then-drain has no TOCTOU:
    // a tx that arrives after this check is simply picked up on
    // the next tick. An empty tick must cost zero L1 RPCs.
    if state
        .lock()
        .expect("sequencer state poisoned")
        .mempool
        .is_empty()
    {
        debug!("batch builder: mempool empty, skipping tick (no L1 RPC)");
        return Ok(None);
    }

    // Read L1-side anchor state. #248: height + state-root come
    // from two separate RPCs; bind them as a consistent pair via
    // a refetch-on-mismatch loop so a mid-read L1 advance can't
    // produce a (height, root) snapshot that never coexisted.
    let (l1_anchor_height, prev_l1_state_root) = fetch_consistent_l1_snapshot(l1).await?;
    let prev_l2_state_root = l1.current_l2_state_root(&cfg.l2_chain_id_hash).await?;

    // Drain the mempool. Hold the lock only long enough to
    // drain — the actual batch construction runs lock-free. The
    // `is_empty` guard below stays as defense even though we
    // peeked above: re-checking after the await is cheap and
    // keeps the drained-empty case correct.
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
    // `MissedTickBehavior::Delay` governs how *late* ticks are
    // rescheduled; it does NOT suppress tokio's immediate first
    // tick (which fires with zero delay). #251: consume that
    // first tick here so startup doesn't immediately try to
    // build a batch before `cfg.interval` has elapsed.
    ticker.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    ticker.tick().await;
    loop {
        select! {
            _ = &mut shutdown => {
                info!("batch builder: shutdown signal received");
                return;
            }
            _ = ticker.tick() => {
                // #245: shutdown preemption. The outer `select!`
                // only checks `shutdown` *between* ticks; once a
                // tick wins, `shutdown` is no longer polled while
                // `run_one_tick` awaits. A stalled L1 RPC would
                // then block Ctrl-C indefinitely. Race the tick's
                // work against `shutdown` so a stall can't pin
                // the loop. Dropping the `run_one_tick` future on
                // shutdown cancels it mid-RPC; because we peek the
                // mempool before draining (and drain only after
                // the L1 reads return), cancellation loses no txs.
                select! {
                    _ = &mut shutdown => {
                        info!("batch builder: shutdown during tick, aborting in-flight work");
                        return;
                    }
                    result = run_one_tick(&state, &cfg, &*l1) => {
                        match result {
                            Ok(_) => {}
                            Err(e) => warn!(error = %e, "batch builder: tick failed"),
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use async_trait::async_trait;

    use super::*;
    use crate::l1_client::mock::MockL1Client;

    /// Bespoke L1 client for #248 snapshot tests. Counts height
    /// reads and can simulate a height that advances between the
    /// initial read and the confirming re-read. Kept local to
    /// this test module so `l1_client.rs` stays untouched.
    struct CountingL1Client {
        /// Current height; bumped by `advance_per_read` on each
        /// `current_l1_height` call.
        height: AtomicU64,
        /// How much to advance the height on every read (0 =
        /// stable L1).
        advance_per_read: u64,
        /// Remaining advances before the L1 settles (used by
        /// `advance_once`); `u64::MAX` for "advance forever".
        advances_left: AtomicU64,
        /// Count of `current_l1_height` calls, for assertions.
        height_reads: AtomicU64,
    }

    /// Deterministic state root for a given L1 height. The
    /// state-root the test client returns is derived FROM the
    /// current height via this function, so a returned
    /// `(height, root)` pair can be checked for internal
    /// consistency: `root == l1_state_root_for_height(height)`.
    /// A client that returned a root belonging to a *different*
    /// height (a torn snapshot) would fail that check.
    fn l1_state_root_for_height(height: u64) -> [u8; 32] {
        [height as u8; 32]
    }

    impl CountingL1Client {
        /// L1 that never moves.
        fn stable(height: u64) -> Self {
            Self {
                height: AtomicU64::new(height),
                advance_per_read: 0,
                advances_left: AtomicU64::new(0),
                height_reads: AtomicU64::new(0),
            }
        }

        /// L1 whose height advances by 1 on every height read and
        /// never settles — the snapshot loop must bail.
        fn ever_advancing(height: u64) -> Self {
            Self {
                height: AtomicU64::new(height),
                advance_per_read: 1,
                advances_left: AtomicU64::new(u64::MAX),
                height_reads: AtomicU64::new(0),
            }
        }

        /// L1 that advances exactly once (on the confirming
        /// re-read) then settles, so one retry recovers.
        fn advance_once(height: u64) -> Self {
            Self {
                height: AtomicU64::new(height),
                advance_per_read: 1,
                advances_left: AtomicU64::new(1),
                height_reads: AtomicU64::new(0),
            }
        }

        fn height_reads(&self) -> u64 {
            self.height_reads.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl L1Client for CountingL1Client {
        async fn current_l1_height(&self) -> Result<u64, L1ClientError> {
            self.height_reads.fetch_add(1, Ordering::SeqCst);
            let h = self.height.load(Ordering::SeqCst);
            if self.advance_per_read > 0
                && self
                    .advances_left
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                        if n == 0 {
                            None
                        } else if n == u64::MAX {
                            Some(u64::MAX)
                        } else {
                            Some(n - 1)
                        }
                    })
                    .is_ok()
            {
                self.height
                    .fetch_add(self.advance_per_read, Ordering::SeqCst);
            }
            Ok(h)
        }

        async fn current_l2_state_root(
            &self,
            _l2_chain_id_hash: &[u8; 32],
        ) -> Result<[u8; 32], L1ClientError> {
            Ok([0u8; 32])
        }

        async fn current_l1_state_root(&self) -> Result<[u8; 32], L1ClientError> {
            // Derive the root from the LIVE height so the
            // returned root always corresponds to whatever
            // height the L1 is currently at. If a height read
            // already advanced the L1, this picks up the new
            // height — exactly the torn-read window the snapshot
            // loop must reconcile.
            let h = self.height.load(Ordering::SeqCst);
            Ok(l1_state_root_for_height(h))
        }

        async fn read_force_include_registry(&self) -> Result<Vec<u8>, L1ClientError> {
            Ok(Vec::new())
        }

        async fn submit_intent(&self, _intent_bytes: Vec<u8>) -> Result<[u8; 32], L1ClientError> {
            Ok([0u8; 32])
        }
    }

    /// Bespoke L1 client for #245: every height read hangs
    /// forever, simulating a stalled L1 RPC. Used to prove the
    /// run loop's inner `select!` lets Ctrl-C preempt a tick
    /// stuck inside `run_one_tick`.
    struct HangingL1Client;

    #[async_trait]
    impl L1Client for HangingL1Client {
        async fn current_l1_height(&self) -> Result<u64, L1ClientError> {
            // Never resolves.
            std::future::pending::<()>().await;
            unreachable!()
        }

        async fn current_l2_state_root(
            &self,
            _l2_chain_id_hash: &[u8; 32],
        ) -> Result<[u8; 32], L1ClientError> {
            std::future::pending::<()>().await;
            unreachable!()
        }

        async fn current_l1_state_root(&self) -> Result<[u8; 32], L1ClientError> {
            std::future::pending::<()>().await;
            unreachable!()
        }

        async fn read_force_include_registry(&self) -> Result<Vec<u8>, L1ClientError> {
            std::future::pending::<()>().await;
            unreachable!()
        }

        async fn submit_intent(&self, _intent_bytes: Vec<u8>) -> Result<[u8; 32], L1ClientError> {
            std::future::pending::<()>().await;
            unreachable!()
        }
    }

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

    /// #249: an empty mempool must skip ALL L1 RPC. We prove this
    /// behaviorally without touching the mock: arm it to fail
    /// every RPC, leave the mempool empty, and assert we still
    /// get `Ok(None)`. If any L1 round-trip ran, it would have
    /// returned `Err` instead.
    #[tokio::test]
    async fn empty_mempool_skips_l1_rpc_entirely() {
        let state = Arc::new(Mutex::new(SequencerState::new()));
        let l1 = MockL1Client::new();
        l1.set_should_fail(true);
        let batch = run_one_tick(&state, &cfg(), &l1).await;
        assert!(
            matches!(batch, Ok(None)),
            "empty tick must not perform any L1 RPC, got {batch:?}"
        );
        assert_eq!(state.lock().unwrap().next_batch_id, 0);
    }

    /// #248: a stable L1 produces a consistent (height, root)
    /// snapshot. `CountingL1Client` records how many height reads
    /// happened; a stable L1 needs exactly one extra confirming
    /// read (read height, read root, re-read height == 2 height
    /// reads, no retry).
    #[tokio::test]
    async fn stable_l1_snapshot_does_not_retry() {
        let l1 = CountingL1Client::stable(100);
        let (h, r) = fetch_consistent_l1_snapshot(&l1).await.unwrap();
        assert_eq!(h, 100);
        // The returned root must be the one that BELONGS to the
        // returned height, not merely some fixed value: prove the
        // pair is internally consistent.
        assert_eq!(
            r,
            l1_state_root_for_height(h),
            "returned root must correspond to the returned height"
        );
        assert_eq!(
            l1.height_reads(),
            2,
            "stable L1 reads height twice, no retry"
        );
    }

    /// #248: if the L1 height advances on every confirming read,
    /// the snapshot loop never stabilizes and must bail with an
    /// `Rpc` error rather than bind a torn (height, root) pair.
    #[tokio::test]
    async fn unstable_l1_height_bails_after_retries() {
        let l1 = CountingL1Client::ever_advancing(100);
        let err = fetch_consistent_l1_snapshot(&l1).await;
        assert!(
            matches!(err, Err(L1ClientError::Rpc(_))),
            "ever-advancing L1 must bail, got {err:?}"
        );
    }

    /// #248: a single mid-read advance is tolerated — the loop
    /// retries with the fresh height and stabilizes.
    #[tokio::test]
    async fn single_l1_advance_recovers_on_retry() {
        let l1 = CountingL1Client::advance_once(100);
        let (h, r) = fetch_consistent_l1_snapshot(&l1).await.unwrap();
        // After one advance the height settles at 101.
        assert_eq!(h, 101);
        // The returned root must track the SETTLED height (101),
        // not the pre-advance height (100): the loop discarded
        // the torn read and rebound the root to the fresh height.
        assert_eq!(
            r,
            l1_state_root_for_height(h),
            "returned root must correspond to the settled height, not the pre-advance one"
        );
        assert_ne!(
            r,
            l1_state_root_for_height(100),
            "the stale pre-advance root must not survive the retry"
        );
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

    /// #245: a tick stuck inside a stalled L1 RPC must NOT block
    /// Ctrl-C. With a `HangingL1Client`, every `run_one_tick`
    /// call parks forever inside the first L1 read; the only way
    /// the loop can exit is if the inner `select!` lets the
    /// shutdown future preempt the in-flight tick. We wrap the
    /// join in a timeout so a regression (blocked shutdown) fails
    /// the test instead of hanging it.
    #[tokio::test]
    async fn run_loop_shutdown_preempts_stalled_l1_rpc() {
        let state = Arc::new(Mutex::new(SequencerState::new()));
        // Non-empty mempool so the tick proceeds past the #249
        // skip-idle guard and into the (hanging) L1 reads.
        state.lock().unwrap().mempool.submit(vec![1, 2, 3]).unwrap();
        let l1 = Arc::new(HangingL1Client);
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let shutdown = Box::pin(async move {
            let _ = shutdown_rx.await;
        });

        let handle = tokio::spawn(run_loop(state, cfg(), l1, shutdown));
        // Give the first real tick time to fire and get stuck in
        // the hanging L1 RPC.
        tokio::time::sleep(Duration::from_millis(120)).await;
        shutdown_tx.send(()).unwrap();
        // If shutdown can't preempt the stalled tick, this join
        // never completes; the timeout converts that hang into a
        // test failure.
        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("shutdown must preempt a stalled L1 RPC")
            .unwrap();
    }

    #[test]
    fn l2_chain_id_hash_derivation_is_deterministic() {
        let a = BatchBuilderTaskConfig::derive_l2_chain_id_hash("gsx-l2-testnet-1");
        let b = BatchBuilderTaskConfig::derive_l2_chain_id_hash("gsx-l2-testnet-1");
        assert_eq!(a, b);
        let c = BatchBuilderTaskConfig::derive_l2_chain_id_hash("gsx-l2-mainnet");
        assert_ne!(a, c);
    }

    // ── Solidity/Rust hash alignment pinned vectors ──────────────
    //
    // These tests lock the exact BLAKE3 recipes the Solidity anchor
    // contract must reproduce. A Solidity implementor can compute
    // `blake3("gsx-l2-chain-" || "test-chain")` and compare against
    // the hex literal below. If either side drifts, the pinned
    // assertion fails immediately.

    /// Pin `l2_chain_id_hash("test-chain")` to a hardcoded hex
    /// digest. Solidity contract: `blake3("gsx-l2-chain-" || chainId)`.
    /// If this test fails, either the Rust recipe changed or the
    /// pinned constant needs updating — either way the Solidity
    /// side must be re-audited.
    #[test]
    fn l2_chain_id_hash_pinned_vector() {
        let hash = BatchBuilderTaskConfig::derive_l2_chain_id_hash("test-chain");
        // Hardcoded reference value. A Solidity implementor can
        // compute blake3("gsx-l2-chain-" || "test-chain") and
        // compare against this constant byte-for-byte.
        assert_eq!(
            hex::encode(hash),
            "46d743898b7c863a8fea1938f261f52134882771b3dd016999964cad793924af",
        );
    }

    /// Pin `da_commitment` = plain `BLAKE3(da_blob)` — no domain
    /// tag. Solidity contract: `blake3(da_blob)`. Exercises the
    /// real `BatchBuilder::build` to prove the production path
    /// emits the same hash an independent `blake3::hash` produces.
    #[test]
    fn da_commitment_matches_plain_blake3() {
        use gsx_l2_sequencer::{BatchBuilder, BuildContext, PendingTx};
        let txs = vec![PendingTx::new(b"test-tx".to_vec()).unwrap()];
        let ctx = BuildContext {
            prev_l2_state_root: [0u8; 32],
            batch_id: 0,
            l1_anchor_height: 0,
            range_vk_commitment: [0u8; 32],
            prev_l1_state_root: [0u8; 32],
            l2_chain_id_hash: [0u8; 32],
            confidential_root: [0u8; 32],
            new_l2_state_root: [0u8; 32],
        };
        let batch = BatchBuilder::build(txs, ctx);
        // Independently compute BLAKE3(da_blob) — must equal the
        // header's da_commitment. If either side adds a domain tag,
        // this assertion catches it.
        let independent = *blake3::hash(&batch.da_blob).as_bytes();
        assert_eq!(batch.header.da_commitment, independent);
    }

    /// `BatchHeader::to_public_inputs()` must be exactly 240 bytes.
    /// The Solidity verifier hard-codes this width.
    #[test]
    fn batch_header_public_inputs_is_240_bytes() {
        use gsx_l2_sequencer::BatchHeader;
        let header = BatchHeader {
            prev_l2_state_root: [0u8; 32],
            new_l2_state_root: [0u8; 32],
            batch_id: 0,
            da_commitment: [0u8; 32],
            l1_anchor_height: 0,
            range_vk_commitment: [0u8; 32],
            prev_l1_state_root: [0u8; 32],
            l2_chain_id_hash: [0u8; 32],
            confidential_root: [0u8; 32],
        };
        let pi = header.to_public_inputs();
        assert_eq!(pi.len(), 240);
    }
}
