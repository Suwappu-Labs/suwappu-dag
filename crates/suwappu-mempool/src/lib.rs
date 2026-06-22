//! Priority + rate-limited mempool for suwappu-dag.
//!
//! Replaces the FIFO `pending_intents: UnboundedSender<Intent>` channel
//! that sits between client ingress (TCP wire + JSON-RPC) and the
//! round-driver's block-proposal site. The original channel had three
//! problems for mainnet:
//!
//! 1. **No fee market.** Every intent has equal priority; submitters
//!    can't pay to jump the queue.
//! 2. **No admission control.** A single misbehaving peer can flood
//!    the channel and stall the round-driver.
//! 3. **No expiry.** A signed intent stuck behind a load spike will
//!    eventually commit at an unrelated time, surprising the
//!    submitter.
//!
//! This crate's `Mempool` fixes all three with a single bounded
//! priority queue plus a per-peer leaky bucket. Defaults are tuned for
//! the perf-cluster topology (4 validators × ~1k intent/s headroom);
//! the daemon owns one `Arc<Mempool>` shared between both ingress
//! sites and the round-driver.
//!
//! ## Architecture
//!
//! ```text
//!     TCP wire ─┐                            ┌── round-driver
//!               ├──► verify_signed_intent ──►│
//!     JSON-RPC ─┘                            │  ┌──────────────────────┐
//!                                            └──┤ Mempool              │
//!                                               │  ├─ priority BTree   │
//!                                               │  ├─ dedup HashSet    │
//!                                               │  └─ per-peer buckets │
//!                                               └──────────────────────┘
//! ```
//!
//! Two methods carry the entire surface:
//!
//! - [`Mempool::submit`] — verified intent + priority + optional peer
//!   label. Returns the canonical intent hash on success.
//! - [`Mempool::drain_for_block`] — round-driver pulls up to N
//!   highest-priority intents at proposal time.
//!
//! See [`Mempool`] for the full API.
//!
//! ## What's deliberately deferred
//!
//! - **The fee unit.** `priority: u64` is opaque to the mempool;
//!   wiring it to a stablecoin fee, byte-size, or anything else is a
//!   policy decision the wire-format PR will make. The mempool is
//!   currency-agnostic on purpose.
//! - **Address-keyed nonces.** Replay protection ultimately lives in
//!   the substrate; the mempool only does content-hash dedup, which
//!   is enough to absorb network retransmissions.
//! - **Persistence across restarts.** Mempool is in-memory; on
//!   restart, peers re-submit. Operators should not rely on the
//!   mempool surviving a daemon bounce.

pub mod bucket;
pub mod mempool;

pub use bucket::LeakyBucket;
pub use mempool::{IntentHash, Mempool, MempoolConfig, MempoolError, MempoolStats};
