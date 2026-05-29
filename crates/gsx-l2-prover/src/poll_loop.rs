//! Prover poll loop (Track G G4.1, #104).
//!
//! Wakes every `poll_interval`, pulls the next unproven batch from the
//! [`UnprovenBatchSource`], proves it via the [`Prover`] with bounded
//! retry/backoff, and submits the resulting proof back to the source.
//! Emits structured tracing + updates the Prometheus [`Metrics`].
//!
//! ## Failure handling
//!
//! - **Transient prove failures** are retried up to
//!   [`RetryPolicy::max_attempts`] with exponential backoff. If the
//!   budget is exhausted, the loop increments `prove_failures_total`,
//!   logs, and continues — it never crashes the daemon.
//! - **Permanent (gated) prove failures** — the [`StubProver`] case —
//!   are not retried: one attempt, then record the failure and move
//!   on. This keeps the loop from spinning against an unwired backend.
//! - **Source errors** (pull / submit) are logged and the tick is
//!   skipped; the next tick retries from a clean slate.
//!
//! ## Testing
//!
//! [`process_one`] is split out from [`run_loop`] so tests drive a
//! single batch deterministically. Backoff sleeps run under
//! `tokio::test(start_paused = true)`, so they auto-advance and the
//! tests don't wait wall-clock time.

use std::{sync::Arc, time::Duration};

use tokio::{select, time};
use tracing::{debug, info, warn};

use crate::{
    metrics::Metrics,
    prover::{Prover, RetryPolicy, UnprovenBatch, UnprovenBatchSource},
};

/// Outcome of [`process_one`], surfaced for tests + tracing.
#[derive(Debug, PartialEq, Eq)]
pub enum TickOutcome {
    /// No batch was available this tick.
    Idle,
    /// A batch was proven + its proof submitted.
    Proven {
        /// The batch id that was proven.
        batch_id: u64,
    },
    /// A batch was pulled but proving exhausted its retry budget
    /// (or was permanently gated). Recorded as a failure; the daemon
    /// continues.
    Failed {
        /// The batch id that failed to prove.
        batch_id: u64,
    },
}

/// Prove a single batch with retry/backoff. Returns `Ok(proof)` on
/// success, or `Err(())` once the retry budget is exhausted (or the
/// error is permanent). Sleeps between attempts per `policy`.
async fn prove_with_retry<P: Prover>(
    prover: &P,
    batch: &UnprovenBatch,
    policy: &RetryPolicy,
    metrics: &Metrics,
) -> Result<crate::prover::Proof, ()> {
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        let backoff = policy.backoff_for(attempt);
        if !backoff.is_zero() {
            debug!(
                batch_id = batch.id,
                attempt,
                backoff_ms = backoff.as_millis() as u64,
                "prover: backing off before retry"
            );
            time::sleep(backoff).await;
        }

        let started = time::Instant::now();
        match prover.prove(batch).await {
            Ok(proof) => {
                let elapsed = started.elapsed().as_secs_f64();
                metrics.record_success(elapsed);
                info!(
                    batch_id = batch.id,
                    attempt,
                    prove_duration_seconds = elapsed,
                    "prover: batch proven"
                );
                return Ok(proof);
            }
            Err(e) if e.is_retryable() && attempt < policy.max_attempts => {
                warn!(
                    batch_id = batch.id,
                    attempt,
                    max_attempts = policy.max_attempts,
                    error = %e,
                    "prover: transient prove failure, will retry"
                );
            }
            Err(e) => {
                // Either permanent (e.g. the SP1 gate) or the retry
                // budget is exhausted. Record + give up on this batch.
                metrics.record_failure();
                warn!(
                    batch_id = batch.id,
                    attempt,
                    retryable = e.is_retryable(),
                    error = %e,
                    "prover: giving up on batch"
                );
                return Err(());
            }
        }
    }
}

/// Process exactly one poll tick: pull a batch, prove it, submit the
/// proof. Broken out from [`run_loop`] so tests drive a single batch.
pub async fn process_one<S, P>(
    source: &S,
    prover: &P,
    policy: &RetryPolicy,
    metrics: &Metrics,
) -> TickOutcome
where
    S: UnprovenBatchSource,
    P: Prover,
{
    let batch = match source.next_unproven_batch().await {
        Ok(Some(b)) => b,
        Ok(None) => {
            debug!("prover: no unproven batch, idling");
            return TickOutcome::Idle;
        }
        Err(e) => {
            warn!(error = %e, "prover: failed to pull next unproven batch");
            return TickOutcome::Idle;
        }
    };

    let batch_id = batch.id;
    match prove_with_retry(prover, &batch, policy, metrics).await {
        Ok(proof) => match source.submit_proof(proof).await {
            Ok(()) => {
                info!(batch_id, "prover: proof submitted");
                TickOutcome::Proven { batch_id }
            }
            Err(e) => {
                // The proof was produced but we couldn't submit it.
                // Count it as a failure so the metric reflects that no
                // proof landed on-chain; the next tick re-pulls.
                metrics.record_failure();
                warn!(batch_id, error = %e, "prover: failed to submit proof");
                TickOutcome::Failed { batch_id }
            }
        },
        Err(()) => TickOutcome::Failed { batch_id },
    }
}

/// Run the poll loop until `shutdown` resolves. Each tick processes at
/// most one batch; the cadence is `poll_interval`.
pub async fn run_loop<S, P, F>(
    source: Arc<S>,
    prover: Arc<P>,
    policy: RetryPolicy,
    metrics: Arc<Metrics>,
    poll_interval: Duration,
    mut shutdown: F,
) where
    S: UnprovenBatchSource,
    P: Prover,
    F: std::future::Future<Output = ()> + Unpin,
{
    let mut ticker = time::interval(poll_interval);
    ticker.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    loop {
        select! {
            _ = &mut shutdown => {
                info!("prover: shutdown signal received");
                return;
            }
            _ = ticker.tick() => {
                let _ = process_one(&*source, &*prover, &policy, &metrics).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prover::{
        mock::{MockBatchSource, MockProver},
        StubProver,
    };

    fn fast_policy() -> RetryPolicy {
        RetryPolicy {
            max_attempts: 5,
            base_backoff: Duration::from_millis(10),
            max_backoff: Duration::from_millis(100),
        }
    }

    fn batch(id: u64) -> UnprovenBatch {
        UnprovenBatch {
            id,
            payload: vec![id as u8; 4],
        }
    }

    /// Success: a single batch proves on the first attempt, the proof
    /// is submitted, and `batches_proven_total` ticks to 1.
    #[tokio::test(start_paused = true)]
    async fn success_proves_and_submits() {
        let source = MockBatchSource::new();
        source.push_batch(batch(1));
        let prover = MockProver::always_succeed();
        let metrics = Metrics::new();

        let outcome = process_one(&source, &prover, &fast_policy(), &metrics).await;

        assert_eq!(outcome, TickOutcome::Proven { batch_id: 1 });
        assert_eq!(prover.attempts(), 1);
        assert_eq!(metrics.batches_proven_total(), 1);
        assert_eq!(metrics.prove_failures_total(), 0);

        let submitted = source.submitted_proofs();
        assert_eq!(submitted.len(), 1);
        assert_eq!(submitted[0].batch_id, 1);
        assert_eq!(submitted[0].bytes, vec![1u8; 4]);
    }

    /// Transient-fail-then-succeed: the prover fails twice (transient),
    /// then succeeds on the third attempt. The loop retries through the
    /// backoff (auto-advanced by the paused clock) and ultimately
    /// records exactly one success and zero permanent failures.
    #[tokio::test(start_paused = true)]
    async fn transient_failures_then_success() {
        let source = MockBatchSource::new();
        source.push_batch(batch(42));
        let prover = MockProver::fail_then_succeed(2);
        let metrics = Metrics::new();

        let outcome = process_one(&source, &prover, &fast_policy(), &metrics).await;

        assert_eq!(outcome, TickOutcome::Proven { batch_id: 42 });
        // 2 transient failures + 1 success = 3 prove calls.
        assert_eq!(prover.attempts(), 3);
        assert_eq!(metrics.batches_proven_total(), 1);
        // Transient retries that eventually succeed are NOT counted as
        // permanent failures.
        assert_eq!(metrics.prove_failures_total(), 0);
        assert_eq!(source.submitted_proofs().len(), 1);
    }

    /// Permanent fail: the transient errors never relent within the
    /// retry budget. The loop gives up after `max_attempts`, records a
    /// failure, submits nothing, and does NOT hang.
    #[tokio::test(start_paused = true)]
    async fn permanent_transient_failure_exhausts_budget() {
        let source = MockBatchSource::new();
        source.push_batch(batch(99));
        // Fail more times than the budget allows.
        let prover = MockProver::fail_then_succeed(100);
        let metrics = Metrics::new();
        let policy = fast_policy();

        let outcome = process_one(&source, &prover, &policy, &metrics).await;

        assert_eq!(outcome, TickOutcome::Failed { batch_id: 99 });
        // Exactly `max_attempts` prove calls, then give up.
        assert_eq!(prover.attempts(), policy.max_attempts);
        assert_eq!(metrics.batches_proven_total(), 0);
        assert_eq!(metrics.prove_failures_total(), 1);
        assert!(source.submitted_proofs().is_empty());
    }

    /// The shipped `StubProver` is permanently gated: one attempt, no
    /// retries (the error is non-retryable), recorded as a failure.
    #[tokio::test(start_paused = true)]
    async fn stub_prover_gate_is_not_retried() {
        let source = MockBatchSource::new();
        source.push_batch(batch(5));
        let prover = StubProver::new();
        let metrics = Metrics::new();

        let outcome = process_one(&source, &prover, &fast_policy(), &metrics).await;

        assert_eq!(outcome, TickOutcome::Failed { batch_id: 5 });
        assert_eq!(metrics.batches_proven_total(), 0);
        assert_eq!(metrics.prove_failures_total(), 1);
        assert!(source.submitted_proofs().is_empty());
    }

    /// Empty source: a tick with no batch idles without touching any
    /// metric.
    #[tokio::test(start_paused = true)]
    async fn empty_source_idles() {
        let source = MockBatchSource::new();
        let prover = MockProver::always_succeed();
        let metrics = Metrics::new();

        let outcome = process_one(&source, &prover, &fast_policy(), &metrics).await;

        assert_eq!(outcome, TickOutcome::Idle);
        assert_eq!(metrics.batches_proven_total(), 0);
        assert_eq!(metrics.prove_failures_total(), 0);
    }

    /// A source pull error is logged + the tick idles (no crash).
    #[tokio::test(start_paused = true)]
    async fn source_pull_error_idles() {
        let source = MockBatchSource::new();
        source.push_batch(batch(1));
        source.set_should_fail(true);
        let prover = MockProver::always_succeed();
        let metrics = Metrics::new();

        let outcome = process_one(&source, &prover, &fast_policy(), &metrics).await;
        assert_eq!(outcome, TickOutcome::Idle);
    }

    /// `run_loop` exits cleanly when the shutdown future resolves.
    #[tokio::test(start_paused = true)]
    async fn run_loop_exits_on_shutdown() {
        let source = Arc::new(MockBatchSource::new());
        let prover = Arc::new(MockProver::always_succeed());
        let metrics = Arc::new(Metrics::new());
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let shutdown = Box::pin(async move {
            let _ = rx.await;
        });

        let handle = tokio::spawn(run_loop(
            source,
            prover,
            fast_policy(),
            metrics,
            Duration::from_millis(50),
            shutdown,
        ));
        // Let a couple ticks fire under the paused clock.
        tokio::time::sleep(Duration::from_millis(120)).await;
        tx.send(()).unwrap();
        handle.await.unwrap();
    }
}
