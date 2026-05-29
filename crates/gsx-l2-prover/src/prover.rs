//! Prover daemon core surfaces: the [`UnprovenBatchSource`] +
//! [`Prover`] traits, the self-contained [`UnprovenBatch`] /
//! [`Proof`] types, and the [`StubProver`] that gates the real SP1
//! proving body (Track G G4.1, #104).
//!
//! ## Why these are traits
//!
//! The daemon's job is to orchestrate a poll loop against two
//! external surfaces:
//!
//! - **Where unproven batches come from + where proofs go** —
//!   [`UnprovenBatchSource`]. The real implementation talks JSON-RPC
//!   to the sequencer daemon (`gsx_getUnprovenBatch` /
//!   `gsx_submitIntent`), but that wiring lands in a follow-up.
//! - **How a batch is proven** — [`Prover`]. The real implementation
//!   runs SP1 over the `gsx-l2-stm` guest and wraps the Plonky3 FRI
//!   proof into Groth16/BN254.
//!
//! Narrowing both behind traits keeps the poll loop testable in
//! isolation (mock source + mock prover) and keeps this crate
//! self-contained — it depends on no other workspace crate.
//!
//! ## The gate
//!
//! The real SP1 proving body is **intentionally absent**. Pulling in
//! `sp1-sdk` / `sp1-zkvm` is the gated #94 decision, and a meaningful
//! guest needs revm-in-guest (#88). Until both land, the shipped
//! [`StubProver`] returns a clear error so an operator who deploys the
//! daemon early sees exactly why no proofs are produced.

use std::time::Duration;

use async_trait::async_trait;
use thiserror::Error;

/// The verbatim error the [`StubProver`] returns. Pinned as a `const`
/// so the gate (#88/#94) is greppable and the unit test asserts on the
/// exact bytes rather than a paraphrase.
pub const SP1_GATE_MESSAGE: &str = "SP1 proving gated on #88 revm-in-guest";

/// A batch awaiting a proof. Deliberately minimal + self-contained:
/// the real `gsx_l2_sequencer::Batch` is richer, but threading it
/// through is follow-up wiring. The scaffold carries only what the
/// poll loop needs to identify the batch and feed the prover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnprovenBatch {
    /// Monotonic batch identifier assigned by the sequencer. Used as
    /// the submit-proof key + the tracing correlation id.
    pub id: u64,
    /// Opaque batch payload (serialized tx list + header). The prover
    /// consumes it; the scaffold treats it as bytes.
    pub payload: Vec<u8>,
}

/// A produced proof. Opaque bytes in the scaffold; the real type is a
/// Groth16/BN254 proof blob plus its public-input commitment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proof {
    /// The batch this proof attests to.
    pub batch_id: u64,
    /// Serialized proof bytes.
    pub bytes: Vec<u8>,
}

/// Errors returned by [`UnprovenBatchSource`] (the JSON-RPC surface in
/// production).
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum SourceError {
    /// Underlying transport (HTTP / WebSocket) error.
    #[error("source transport error: {0}")]
    Transport(String),

    /// Source returned a JSON-RPC error response.
    #[error("source rpc error: {0}")]
    Rpc(String),
}

/// Errors returned by [`Prover`].
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ProveError {
    /// The proving backend is not yet wired (the [`StubProver`] case,
    /// gated on #88 / #94). Treated as **permanent** by the poll loop:
    /// retrying will never succeed, so the loop records a failure and
    /// moves on rather than spinning.
    #[error("{0}")]
    Gated(String),

    /// A transient proving failure (OOM, prover process crash, GPU
    /// hiccup). The poll loop retries these with backoff.
    #[error("transient prove failure: {0}")]
    Transient(String),
}

impl ProveError {
    /// Whether the poll loop should retry this error. `Gated` is
    /// permanent (no amount of retrying wires up SP1); `Transient`
    /// is retryable.
    pub fn is_retryable(&self) -> bool {
        matches!(self, ProveError::Transient(_))
    }
}

/// Source of unproven batches + sink for produced proofs.
///
/// `Send + Sync + 'static` because the daemon holds it across Tokio
/// tasks. The real implementation wraps a JSON-RPC client to the
/// sequencer daemon; tests use [`mock::MockBatchSource`].
#[async_trait]
pub trait UnprovenBatchSource: Send + Sync + 'static {
    /// Pull the next unproven batch, or `None` if the queue is empty.
    async fn next_unproven_batch(&self) -> Result<Option<UnprovenBatch>, SourceError>;

    /// Submit a produced proof back to the source (in production, an
    /// `Intent::CommitL2StateRoot` via `gsx_submitIntent`).
    async fn submit_proof(&self, proof: Proof) -> Result<(), SourceError>;
}

/// Proving backend.
///
/// `Send + Sync + 'static` for the same task-sharing reason as
/// [`UnprovenBatchSource`]. The only shipped implementation is
/// [`StubProver`]; the real SP1 backend is gated on #88 / #94.
#[async_trait]
pub trait Prover: Send + Sync + 'static {
    /// Produce a proof for `batch`. Errors are classified by
    /// [`ProveError::is_retryable`] to drive the poll loop's
    /// retry/backoff decision.
    async fn prove(&self, batch: &UnprovenBatch) -> Result<Proof, ProveError>;
}

/// The shipped, gated prover. Always returns
/// [`ProveError::Gated`] carrying [`SP1_GATE_MESSAGE`].
///
/// This is the SCAFFOLD boundary: the real SP1 proving body lives
/// behind the [`Prover`] trait and is intentionally not implemented
/// here. Deploying the daemon with `StubProver` produces no proofs,
/// only a clear, repeated log line explaining the gate — which is the
/// correct behavior until #88 (revm-in-guest) and the #94 sp1-sdk
/// decision land.
#[derive(Debug, Default, Clone)]
pub struct StubProver;

impl StubProver {
    /// Construct the stub prover.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Prover for StubProver {
    async fn prove(&self, _batch: &UnprovenBatch) -> Result<Proof, ProveError> {
        Err(ProveError::Gated(SP1_GATE_MESSAGE.to_string()))
    }
}

/// Retry / backoff policy for the poll loop.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Maximum proving attempts per batch before recording a
    /// permanent failure and moving on. `1` means no retry.
    pub max_attempts: u32,
    /// Base backoff delay. Attempt `n` (1-indexed) sleeps
    /// `base * 2^(n-1)`, capped at `max_backoff`.
    pub base_backoff: Duration,
    /// Cap on the exponential backoff delay.
    pub max_backoff: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            base_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(30),
        }
    }
}

impl RetryPolicy {
    /// Backoff delay before attempt `attempt` (1-indexed). Attempt 1
    /// has no preceding backoff (returns `Duration::ZERO`); attempt
    /// `n > 1` waits `base * 2^(n-2)`, saturating at `max_backoff`.
    pub fn backoff_for(&self, attempt: u32) -> Duration {
        if attempt <= 1 {
            return Duration::ZERO;
        }
        let shift = attempt - 2;
        // Saturating doubling so a large `max_attempts` can't overflow
        // the multiplier; the `min` with `max_backoff` caps it anyway.
        let factor = 1u64.checked_shl(shift).unwrap_or(u64::MAX);
        let scaled = self
            .base_backoff
            .saturating_mul(factor.min(u32::MAX as u64) as u32);
        scaled.min(self.max_backoff)
    }
}

/// In-memory mocks for unit-testing the poll loop without real I/O.
pub mod mock {
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::{Proof, ProveError, Prover, SourceError, UnprovenBatch, UnprovenBatchSource};

    /// Mock batch source: hands out a pre-seeded FIFO queue of
    /// batches and records every submitted proof.
    #[derive(Debug, Default)]
    pub struct MockBatchSource {
        inner: Mutex<MockSourceState>,
    }

    #[derive(Debug, Default)]
    struct MockSourceState {
        /// Pending batches, drained front-to-back by
        /// `next_unproven_batch`.
        queue: std::collections::VecDeque<UnprovenBatch>,
        /// Proofs submitted via `submit_proof`, in order.
        submitted: Vec<Proof>,
        /// When set, every method returns `SourceError::Rpc`.
        should_fail: bool,
    }

    impl MockBatchSource {
        /// Empty source (no batches, no failures).
        pub fn new() -> Self {
            Self::default()
        }

        /// Seed a batch onto the back of the queue.
        pub fn push_batch(&self, batch: UnprovenBatch) {
            self.inner.lock().unwrap().queue.push_back(batch);
        }

        /// Read all proofs submitted so far.
        pub fn submitted_proofs(&self) -> Vec<Proof> {
            self.inner.lock().unwrap().submitted.clone()
        }

        /// Make every method return `SourceError::Rpc`.
        pub fn set_should_fail(&self, fail: bool) {
            self.inner.lock().unwrap().should_fail = fail;
        }
    }

    #[async_trait]
    impl UnprovenBatchSource for MockBatchSource {
        async fn next_unproven_batch(&self) -> Result<Option<UnprovenBatch>, SourceError> {
            let mut s = self.inner.lock().unwrap();
            if s.should_fail {
                return Err(SourceError::Rpc("mock source failure".into()));
            }
            Ok(s.queue.pop_front())
        }

        async fn submit_proof(&self, proof: Proof) -> Result<(), SourceError> {
            let mut s = self.inner.lock().unwrap();
            if s.should_fail {
                return Err(SourceError::Rpc("mock source failure".into()));
            }
            s.submitted.push(proof);
            Ok(())
        }
    }

    /// Mock prover with scriptable behavior: fail the first `fail_n`
    /// attempts (each a transient error), then succeed. `fail_n = 0`
    /// succeeds immediately; a large `fail_n` models a permanent
    /// failure that exhausts any finite retry budget.
    #[derive(Debug)]
    pub struct MockProver {
        inner: Mutex<MockProverState>,
    }

    #[derive(Debug)]
    struct MockProverState {
        /// Remaining transient failures to emit before succeeding.
        fail_n: u32,
        /// Total `prove` calls observed (for assertions).
        attempts: u32,
    }

    impl MockProver {
        /// Succeed immediately (no transient failures).
        pub fn always_succeed() -> Self {
            Self::fail_then_succeed(0)
        }

        /// Fail the first `fail_n` attempts (transient), then succeed.
        pub fn fail_then_succeed(fail_n: u32) -> Self {
            Self {
                inner: Mutex::new(MockProverState {
                    fail_n,
                    attempts: 0,
                }),
            }
        }

        /// Total `prove` calls observed so far.
        pub fn attempts(&self) -> u32 {
            self.inner.lock().unwrap().attempts
        }
    }

    #[async_trait]
    impl Prover for MockProver {
        async fn prove(&self, batch: &UnprovenBatch) -> Result<Proof, ProveError> {
            let mut s = self.inner.lock().unwrap();
            s.attempts += 1;
            if s.fail_n > 0 {
                s.fail_n -= 1;
                return Err(ProveError::Transient(format!(
                    "mock transient failure (remaining {})",
                    s.fail_n
                )));
            }
            Ok(Proof {
                batch_id: batch.id,
                bytes: batch.payload.clone(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stub_prover_returns_the_exact_gate_message() {
        let prover = StubProver::new();
        let batch = UnprovenBatch {
            id: 7,
            payload: vec![1, 2, 3],
        };
        let err = prover.prove(&batch).await.unwrap_err();
        assert_eq!(err, ProveError::Gated(SP1_GATE_MESSAGE.to_string()));
        // The verbatim string is part of the #104 contract.
        assert_eq!(err.to_string(), "SP1 proving gated on #88 revm-in-guest");
        // And it is permanent: the loop must not retry it forever.
        assert!(!err.is_retryable());
    }

    #[test]
    fn prove_error_retry_classification() {
        assert!(ProveError::Transient("x".into()).is_retryable());
        assert!(!ProveError::Gated("y".into()).is_retryable());
    }

    #[test]
    fn backoff_is_exponential_and_capped() {
        let policy = RetryPolicy {
            max_attempts: 10,
            base_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_millis(1000),
        };
        // Attempt 1 has no preceding backoff.
        assert_eq!(policy.backoff_for(1), Duration::ZERO);
        // 100, 200, 400, 800, then capped at 1000.
        assert_eq!(policy.backoff_for(2), Duration::from_millis(100));
        assert_eq!(policy.backoff_for(3), Duration::from_millis(200));
        assert_eq!(policy.backoff_for(4), Duration::from_millis(400));
        assert_eq!(policy.backoff_for(5), Duration::from_millis(800));
        assert_eq!(policy.backoff_for(6), Duration::from_millis(1000));
        // Large attempt numbers stay capped (no overflow panic).
        assert_eq!(policy.backoff_for(64), Duration::from_millis(1000));
    }
}
