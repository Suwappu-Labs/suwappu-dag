//! gsx-l2-prover — the L2 prover daemon's Tokio + RPC shell.
//! Track G G4.1 (#104).
//!
//! ## What this daemon does
//!
//! A long-running poll loop that, each tick:
//!
//! 1. pulls the next unproven batch from an [`UnprovenBatchSource`],
//! 2. proves it via a [`Prover`] (with bounded retry + exponential
//!    backoff on transient failures),
//! 3. submits the resulting [`Proof`] back to the source.
//!
//! It emits structured tracing and a Prometheus `/metrics` endpoint
//! exposing `prove_duration_seconds`, `batches_proven_total`, and
//! `prove_failures_total`.
//!
//! ## SCAFFOLD boundary
//!
//! The **real SP1 proving body is gated** behind the [`Prover`] trait.
//! The only shipped implementation is [`StubProver`], which returns
//! [`prover::SP1_GATE_MESSAGE`] (`"SP1 proving gated on #88
//! revm-in-guest"`). Wiring the real prover requires:
//!
//! - **#88** — revm-in-guest, so the `gsx-l2-stm` guest can execute
//!   the batch's tx list, and
//! - **#94** — the decision to take the `sp1-sdk` / `sp1-zkvm`
//!   dependency.
//!
//! Neither dep is added here on purpose. The scaffold is fully
//! testable end-to-end (poll loop + retry/backoff + metrics) against
//! mock implementations, and deploying it early yields a clear,
//! repeated log line explaining why no proofs are produced rather than
//! a silent no-op.
//!
//! ## Crate layout
//!
//! - [`config`] — TOML config + validation.
//! - [`prover`] — the two traits, the self-contained batch/proof
//!   types, [`StubProver`], the retry policy, and test mocks.
//! - [`poll_loop`] — the tick logic + the shutdown-aware run loop.
//! - [`metrics`] — the hand-rolled Prometheus metric set.
//! - [`metrics_http`] — the thin axum `/metrics` server.
//!
//! The crate is deliberately **self-contained**: it depends on no
//! other workspace crate. The real `UnprovenBatchSource`
//! (sequencer JSON-RPC client) + `Prover` (SP1) implementations land
//! in follow-ups once their gates clear.

pub mod config;
pub mod metrics;
pub mod metrics_http;
pub mod poll_loop;
pub mod prover;

pub use config::{ConfigError, ProverConfig};
pub use metrics::Metrics;
pub use poll_loop::{process_one, run_loop, TickOutcome};
pub use prover::{
    Proof, ProveError, Prover, RetryPolicy, SourceError, StubProver, UnprovenBatch,
    UnprovenBatchSource, SP1_GATE_MESSAGE,
};
