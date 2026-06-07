//! `suwappu-indexer` — streaming indexer for suwappu-dag.
//!
//! ## Architecture
//!
//! ```text
//!  ┌──────────────┐  WS /ws         ┌──────────────┐  HTTP GET     ┌──────────┐
//!  │ suwappu-node     │ ───────────────▶│ suwappu-indexer  │ ◀────────────▶│ explorer │
//!  │ (validator)  │  EventView JSON │ (this crate) │   /blocks…    │  (TS UI) │
//!  └──────────────┘                 └──────────────┘                └──────────┘
//!         ▲                                │
//!         │ HTTP POST /                    ▼
//!         │ suwappu_getBlock(round)     ┌──────────────┐
//!         └─────────────────────────│  in-mem +    │
//!           catch-up backfill       │  Postgres    │   ← future
//!                                   └──────────────┘
//! ```
//!
//! ## Mode
//!
//! T6 (this PR) ships the **scaffold**: WebSocket subscriber + in-memory
//! store + thin HTTP read API. T7 (follow-up) adds:
//!
//! - Postgres persistence (sqlx migrations).
//! - Catch-up backfill via `suwappu_getBlock(round)` from the last
//!   checkpointed round.
//! - Idempotent restart (resume from checkpoint).
//!
//! ## Module map
//!
//! - [`store`] — pluggable backing store. In-memory MVP; trait shape
//!   so the Postgres adapter slots in without changing call sites.
//! - [`subscriber`] — WebSocket client. Pulls `EventView` frames,
//!   normalizes lagged-frame notices, and feeds the store.
//! - [`api`] — HTTP read API surface (axum). Mirrors `suwappu-rpc`'s
//!   `suwappu_getBlock` / `suwappu_getTransaction` shape for client parity.
//! - [`config`] — CLI flags and runtime config.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod api;
pub mod backfill;
pub mod config;
pub mod store;
pub mod subscriber;

#[cfg(feature = "postgres")]
pub mod postgres;

pub use backfill::catch_up;
pub use config::IndexerConfig;
#[cfg(feature = "postgres")]
pub use postgres::PostgresStore;
pub use store::{InMemoryStore, IndexedBlock, Store};
