//! Deterministic matching engine: one instance per market.

use serde::{Deserialize, Serialize};

use crate::book::{OrderBook, RestingOrder};
use crate::types::{AccountId, ClobError, Fill, MarketId, NewOrder, OrderId, TimeInForce};

/// What the engine does when an incoming order would trade against a
/// resting order from the same account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelfTradePolicy {
    /// Cancel the remaining taker quantity; the resting order stays.
    CancelTaker,
    /// Cancel the resting order and keep matching the taker.
    CancelResting,
}

/// Terminal disposition of a submitted order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderStatus {
    /// Fully filled on entry.
    Filled,
    /// Partially filled; the remainder rests on the book.
    PartiallyFilledResting,
    /// No fill; the full quantity rests on the book.
    Resting,
    /// Partially filled; the remainder was canceled (IOC or self-trade).
    PartiallyFilledCanceled,
    /// Nothing filled and nothing resting (IOC miss, FOK kill, post-only
    /// cross, or self-trade cancel before any fill).
    Canceled,
}

/// Result of submitting one order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitOutcome {
    /// Engine-assigned id for the submitted order.
    pub order_id: OrderId,
    /// Fills generated, in execution order.
    pub fills: Vec<Fill>,
    /// Resting orders canceled by self-trade prevention.
    pub canceled_resting: Vec<OrderId>,
    /// Quantity left resting on the book (0 unless status is a resting one).
    pub resting_qty: u64,
    /// Terminal disposition.
    pub status: OrderStatus,
}

/// A single-market matching engine.
///
/// Holds no clock and no randomness: order ids and fill sequence numbers are
/// engine-assigned monotone counters, so replaying the same submission
/// stream reproduces identical fills and an identical book on every
/// validator.
#[derive(Debug)]
pub struct MatchingEngine {
    market: MarketId,
    book: OrderBook,
    stp: SelfTradePolicy,
    next_order_id: u64,
    next_fill_seq: u64,
}

impl MatchingEngine {
    /// New engine for `market` with the given self-trade policy.
    pub fn new(market: MarketId, stp: SelfTradePolicy) -> Self {
        Self {
            market,
            book: OrderBook::new(),
            stp,
            next_order_id: 0,
            next_fill_seq: 0,
        }
    }

    /// The market this engine serves.
    pub fn market(&self) -> MarketId {
        self.market
    }

    /// Read-only view of the book.
    pub fn book(&self) -> &OrderBook {
        &self.book
    }

    /// Submit an order; returns fills and the order's disposition.
    pub fn submit(&mut self, order: NewOrder) -> Result<SubmitOutcome, ClobError> {
        if order.price == 0 || order.qty == 0 {
            return Err(ClobError::ZeroPriceOrQty);
        }
        if order.market != self.market {
            return Err(ClobError::WrongMarket);
        }

        let order_id = OrderId(self.next_order_id);
        self.next_order_id += 1;

        // Post-only: reject on any cross, otherwise rest untouched.
        if order.tif == TimeInForce::PostOnly {
            if self.book.would_cross(order.side, order.price) {
                return Ok(SubmitOutcome {
                    order_id,
                    fills: Vec::new(),
                    canceled_resting: Vec::new(),
                    resting_qty: 0,
                    status: OrderStatus::Canceled,
                });
            }
            self.rest(order_id, &order, order.qty);
            return Ok(SubmitOutcome {
                order_id,
                fills: Vec::new(),
                canceled_resting: Vec::new(),
                resting_qty: order.qty,
                status: OrderStatus::Resting,
            });
        }

        // Fill-or-kill: atomic liquidity pre-check against non-self depth.
        if order.tif == TimeInForce::FillOrKill {
            let available = self
                .book
                .crossable_qty(order.side, order.price, order.account);
            if available < u128::from(order.qty) {
                return Ok(SubmitOutcome {
                    order_id,
                    fills: Vec::new(),
                    canceled_resting: Vec::new(),
                    resting_qty: 0,
                    status: OrderStatus::Canceled,
                });
            }
        }

        let mut remaining = order.qty;
        let mut fills = Vec::new();
        let mut canceled_resting = Vec::new();
        let mut taker_canceled = false;

        while remaining > 0 {
            let Some((price, maker)) = self.book.best_counter(order.side, order.price) else {
                break;
            };
            if maker.account == order.account {
                match self.stp {
                    SelfTradePolicy::CancelTaker => {
                        taker_canceled = true;
                        break;
                    }
                    SelfTradePolicy::CancelResting => {
                        self.book.remove(maker.id);
                        canceled_resting.push(maker.id);
                        continue;
                    }
                }
            }
            let qty = remaining.min(maker.qty);
            self.book.consume_front(order.side.opposite(), price, qty);
            fills.push(Fill {
                maker_order: maker.id,
                taker_order: order_id,
                maker_account: maker.account,
                taker_account: order.account,
                price,
                qty,
                taker_side: order.side,
                seq: self.next_fill_seq,
            });
            self.next_fill_seq += 1;
            remaining -= qty;
        }

        let filled_any = !fills.is_empty();
        let status = if remaining == 0 {
            OrderStatus::Filled
        } else if taker_canceled {
            if filled_any {
                OrderStatus::PartiallyFilledCanceled
            } else {
                OrderStatus::Canceled
            }
        } else {
            match order.tif {
                TimeInForce::GoodTillCancel => {
                    self.rest(order_id, &order, remaining);
                    if filled_any {
                        OrderStatus::PartiallyFilledResting
                    } else {
                        OrderStatus::Resting
                    }
                }
                TimeInForce::ImmediateOrCancel | TimeInForce::FillOrKill => {
                    if filled_any {
                        OrderStatus::PartiallyFilledCanceled
                    } else {
                        OrderStatus::Canceled
                    }
                }
                TimeInForce::PostOnly => unreachable!("post-only handled above"),
            }
        };

        let resting_qty = match status {
            OrderStatus::Resting | OrderStatus::PartiallyFilledResting => remaining,
            _ => 0,
        };
        Ok(SubmitOutcome {
            order_id,
            fills,
            canceled_resting,
            resting_qty,
            status,
        })
    }

    /// Cancel a resting order. Only the owning account may cancel.
    pub fn cancel(&mut self, id: OrderId, account: AccountId) -> Result<u64, ClobError> {
        match self.book.owner_of(id) {
            None => Err(ClobError::UnknownOrder),
            Some(owner) if owner != account => Err(ClobError::NotOrderOwner),
            Some(_) => Ok(self.book.remove(id).map(|o| o.qty).unwrap_or(0)),
        }
    }

    fn rest(&mut self, id: OrderId, order: &NewOrder, qty: u64) {
        self.book.insert(
            order.side,
            order.price,
            RestingOrder {
                id,
                account: order.account,
                qty,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Side;

    fn mkt() -> MarketId {
        MarketId([7; 32])
    }
    fn acct(b: u8) -> AccountId {
        AccountId([b; 32])
    }
    fn order(account: u8, side: Side, price: u64, qty: u64, tif: TimeInForce) -> NewOrder {
        NewOrder {
            market: mkt(),
            account: acct(account),
            side,
            price,
            qty,
            tif,
        }
    }

    #[test]
    fn crossing_fills_at_maker_price_in_time_priority() {
        let mut e = MatchingEngine::new(mkt(), SelfTradePolicy::CancelTaker);
        e.submit(order(1, Side::Ask, 100, 5, TimeInForce::GoodTillCancel))
            .unwrap();
        e.submit(order(2, Side::Ask, 100, 5, TimeInForce::GoodTillCancel))
            .unwrap();
        let out = e
            .submit(order(3, Side::Bid, 101, 8, TimeInForce::GoodTillCancel))
            .unwrap();
        assert_eq!(out.status, OrderStatus::Filled);
        assert_eq!(out.fills.len(), 2);
        // Maker price, not taker limit.
        assert!(out.fills.iter().all(|f| f.price == 100));
        // Earlier ask fills first and fully.
        assert_eq!(out.fills[0].maker_account, acct(1));
        assert_eq!(out.fills[0].qty, 5);
        assert_eq!(out.fills[1].qty, 3);
        // Remainder of maker 2 still resting.
        assert_eq!(e.book().depth(), 1);
    }

    #[test]
    fn ioc_cancels_remainder_and_gtc_rests_it() {
        let mut e = MatchingEngine::new(mkt(), SelfTradePolicy::CancelTaker);
        e.submit(order(1, Side::Ask, 100, 4, TimeInForce::GoodTillCancel))
            .unwrap();
        let ioc = e
            .submit(order(2, Side::Bid, 100, 10, TimeInForce::ImmediateOrCancel))
            .unwrap();
        assert_eq!(ioc.status, OrderStatus::PartiallyFilledCanceled);
        assert_eq!(ioc.fills[0].qty, 4);
        assert_eq!(e.book().depth(), 0);
        let gtc = e
            .submit(order(3, Side::Bid, 100, 10, TimeInForce::GoodTillCancel))
            .unwrap();
        assert_eq!(gtc.status, OrderStatus::Resting);
        assert_eq!(gtc.resting_qty, 10);
    }

    #[test]
    fn fok_is_atomic() {
        let mut e = MatchingEngine::new(mkt(), SelfTradePolicy::CancelTaker);
        e.submit(order(1, Side::Ask, 100, 4, TimeInForce::GoodTillCancel))
            .unwrap();
        let kill = e
            .submit(order(2, Side::Bid, 100, 5, TimeInForce::FillOrKill))
            .unwrap();
        assert_eq!(kill.status, OrderStatus::Canceled);
        assert!(kill.fills.is_empty());
        assert_eq!(e.book().depth(), 1);
        let fill = e
            .submit(order(2, Side::Bid, 100, 4, TimeInForce::FillOrKill))
            .unwrap();
        assert_eq!(fill.status, OrderStatus::Filled);
    }

    #[test]
    fn post_only_rejects_cross_and_rests_otherwise() {
        let mut e = MatchingEngine::new(mkt(), SelfTradePolicy::CancelTaker);
        e.submit(order(1, Side::Ask, 100, 4, TimeInForce::GoodTillCancel))
            .unwrap();
        let rejected = e
            .submit(order(2, Side::Bid, 100, 4, TimeInForce::PostOnly))
            .unwrap();
        assert_eq!(rejected.status, OrderStatus::Canceled);
        let rested = e
            .submit(order(2, Side::Bid, 99, 4, TimeInForce::PostOnly))
            .unwrap();
        assert_eq!(rested.status, OrderStatus::Resting);
    }

    #[test]
    fn self_trade_policies() {
        // CancelTaker: incoming order stops at own resting order.
        let mut e = MatchingEngine::new(mkt(), SelfTradePolicy::CancelTaker);
        e.submit(order(1, Side::Ask, 100, 4, TimeInForce::GoodTillCancel))
            .unwrap();
        let out = e
            .submit(order(1, Side::Bid, 100, 4, TimeInForce::GoodTillCancel))
            .unwrap();
        assert_eq!(out.status, OrderStatus::Canceled);
        assert!(out.fills.is_empty());
        assert_eq!(e.book().depth(), 1);

        // CancelResting: own resting order is canceled, matching continues.
        let mut e = MatchingEngine::new(mkt(), SelfTradePolicy::CancelResting);
        let own = e
            .submit(order(1, Side::Ask, 100, 4, TimeInForce::GoodTillCancel))
            .unwrap();
        e.submit(order(2, Side::Ask, 100, 4, TimeInForce::GoodTillCancel))
            .unwrap();
        let out = e
            .submit(order(1, Side::Bid, 100, 4, TimeInForce::GoodTillCancel))
            .unwrap();
        assert_eq!(out.canceled_resting, vec![own.order_id]);
        assert_eq!(out.fills.len(), 1);
        assert_eq!(out.fills[0].maker_account, acct(2));
        assert_eq!(out.status, OrderStatus::Filled);
    }

    #[test]
    fn cancel_requires_ownership() {
        let mut e = MatchingEngine::new(mkt(), SelfTradePolicy::CancelTaker);
        let out = e
            .submit(order(1, Side::Ask, 100, 4, TimeInForce::GoodTillCancel))
            .unwrap();
        assert_eq!(
            e.cancel(out.order_id, acct(2)),
            Err(ClobError::NotOrderOwner)
        );
        assert_eq!(e.cancel(out.order_id, acct(1)), Ok(4));
        assert_eq!(
            e.cancel(out.order_id, acct(1)),
            Err(ClobError::UnknownOrder)
        );
    }

    #[test]
    fn replay_determinism() {
        let stream: Vec<NewOrder> = vec![
            order(1, Side::Ask, 102, 5, TimeInForce::GoodTillCancel),
            order(2, Side::Ask, 101, 3, TimeInForce::GoodTillCancel),
            order(3, Side::Bid, 103, 6, TimeInForce::GoodTillCancel),
            order(4, Side::Bid, 100, 2, TimeInForce::PostOnly),
            order(5, Side::Ask, 100, 9, TimeInForce::ImmediateOrCancel),
        ];
        let run = |orders: &[NewOrder]| {
            let mut e = MatchingEngine::new(mkt(), SelfTradePolicy::CancelTaker);
            orders
                .iter()
                .map(|o| e.submit(*o).unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(run(&stream), run(&stream));
    }
}
