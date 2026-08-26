//! Core order, fill, and identifier types for the CLOB lane.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A market (trading pair) identifier — SHA3-256 of the canonical pair
/// descriptor, assigned at market creation by the registered-issuer path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MarketId(pub [u8; 32]);

/// An account identifier — the same 32-byte address space the execution
/// substrate uses for balance-map keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AccountId(pub [u8; 32]);

/// Engine-assigned order identifier, monotone in submission order within a
/// market. Doubles as the time-priority key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct OrderId(pub u64);

/// Which side of the book an order rests on or takes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    /// Buy base with quote.
    Bid,
    /// Sell base for quote.
    Ask,
}

impl Side {
    /// The side this order matches against.
    pub fn opposite(self) -> Side {
        match self {
            Side::Bid => Side::Ask,
            Side::Ask => Side::Bid,
        }
    }
}

/// Time-in-force semantics for a new order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimeInForce {
    /// Match what crosses; rest the remainder on the book.
    GoodTillCancel,
    /// Match what crosses; cancel the remainder.
    ImmediateOrCancel,
    /// Match the full quantity atomically or match nothing.
    FillOrKill,
    /// Rest on the book only; reject if any part would cross.
    PostOnly,
}

/// A new order as submitted by a venue, market maker, or client lane.
///
/// Prices are integer ticks and quantities integer lots; the market's tick
/// and lot sizes are fixed at market creation, so the engine never touches
/// floating point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewOrder {
    /// Market the order targets.
    pub market: MarketId,
    /// Submitting account.
    pub account: AccountId,
    /// Bid or ask.
    pub side: Side,
    /// Limit price in ticks. Must be non-zero.
    pub price: u64,
    /// Quantity in lots. Must be non-zero.
    pub qty: u64,
    /// Time-in-force semantics.
    pub tif: TimeInForce,
}

/// One maker/taker execution produced by the matching engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fill {
    /// Resting order that provided liquidity.
    pub maker_order: OrderId,
    /// Incoming order that took liquidity.
    pub taker_order: OrderId,
    /// Account behind the maker order.
    pub maker_account: AccountId,
    /// Account behind the taker order.
    pub taker_account: AccountId,
    /// Execution price in ticks — always the maker's resting price.
    pub price: u64,
    /// Executed quantity in lots.
    pub qty: u64,
    /// Side of the taker (the maker is on the opposite side).
    pub taker_side: Side,
    /// Engine-assigned fill sequence number, monotone within a market.
    pub seq: u64,
}

/// Errors surfaced at the engine boundary.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ClobError {
    /// Order price or quantity was zero.
    #[error("order price and quantity must be non-zero")]
    ZeroPriceOrQty,
    /// Order targeted a different market than this engine instance.
    #[error("order market does not match engine market")]
    WrongMarket,
    /// Cancel referenced an order that is not resting.
    #[error("unknown or already-filled order")]
    UnknownOrder,
    /// Cancel came from an account that does not own the order.
    #[error("order is owned by a different account")]
    NotOrderOwner,
    /// Notional (price × qty) overflowed the settlement accumulator.
    #[error("notional overflow")]
    NotionalOverflow,
}
