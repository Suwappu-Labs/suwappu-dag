//! suwappu-clob — the headless exchange lane of the SUWAPPU DAG L1.
//!
//! Collapses the four functions a traditional venue splits across systems
//! into one deterministic, chain-native stack:
//!
//! - **Matching** — price-time-priority central limit order book
//!   ([`engine::MatchingEngine`]), fully deterministic: same order stream in,
//!   same fill stream out, on every validator.
//! - **Clearing** — per-account multilateral netting of fills into signed
//!   base/quote deltas ([`settlement::SettlementBatch`]).
//! - **Settlement** — a constant-size SHA3-256 batch root
//!   ([`settlement::SettlementBatch::batch_root`]) that the execution
//!   substrate applies and the LTP anchor pipeline attests cross-chain.
//! - **Risk** — self-trade prevention, fill-or-kill liquidity pre-checks,
//!   post-only crossing rejection, and checked notional arithmetic at the
//!   engine boundary.
//!
//! No consumer frontend lives here: venues, market creators, and market
//! makers submit orders through the RPC/mempool lanes and receive fills and
//! settlement roots. The Suwappu bot and webapp are just the first clients.
//!
//! Determinism is the load-bearing property: the engine holds no clocks and
//! no randomness. Time priority is the engine-assigned submission sequence,
//! so replaying a certified order stream reproduces the book, the fills, and
//! the settlement root byte-for-byte — which is what lets matching live
//! inside consensus instead of beside it.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod book;
pub mod engine;
pub mod settlement;
pub mod types;

pub use book::OrderBook;
pub use engine::{MatchingEngine, SubmitOutcome};
pub use engine::{OrderStatus, SelfTradePolicy};
pub use settlement::{AccountDelta, SettlementBatch};
pub use types::{AccountId, ClobError, Fill, MarketId, NewOrder, OrderId, Side, TimeInForce};
