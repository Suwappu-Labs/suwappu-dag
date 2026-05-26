//! gsx-l2-sequencer-daemon — the L2 sequencer's Tokio + RPC
//! shell. Phase 2.2 / Track G G4.2 (#105).
//!
//! This crate is the **wiring layer**. The interesting
//! correctness lives in:
//!
//! - [`gsx_l2_sequencer`] — `TxMempool`, `BatchBuilder`,
//!   `force_include::evaluate`. All pure logic, fully tested.
//! - [`gsx_l2_stm`] — `execute_batch`, `to_public_inputs`.
//!   Native reference STM; the SP1 guest shares the same lib.
//!
//! The daemon's job is to orchestrate those crates against
//! real I/O: a configurable Tokio runtime, a JSON-RPC server
//! for L2 tx submission, an L1 substrate client for posting
//! `Intent::PostL2DA` + `Intent::CommitL2StateRoot` +
//! `Intent::MarkForceIncludeHonored` etc., and a tracing
//! subscriber for ops visibility.
//!
//! ## Phase split
//!
//! - **Phase 2.2-a (this commit)** — Cargo crate + config +
//!   CLI + tokio runtime + tracing init. No I/O tasks yet.
//! - **Phase 2.2-b** — batch-builder tick (timer-driven) +
//!   stub `L1Client` trait.
//! - **Phase 2.2-c** — force-include watcher + real RPC.
//! - **Phase 2.2-d** — JSON-RPC server (`l2_sendRawTransaction`,
//!   `l2_getBalance`, `l2_getTransactionReceipt`).
//! - **Phase 2.2-e** — terraform/testnet/l2.tf + systemd unit
//!   + OPERATIONS.md § 10.6 (deploy runbook).
//!
//! Splitting this way lets each follow-up land in a focused
//! PR that exercises one I/O surface.

pub mod batch_builder_task;
pub mod config;
pub mod l1_client;

pub use batch_builder_task::{BatchBuilderTaskConfig, SequencerState};
pub use config::{ConfigError, SequencerConfig};
pub use l1_client::{L1Client, L1ClientError};
