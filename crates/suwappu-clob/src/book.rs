//! Price-time-priority order book.
//!
//! Storage is `BTreeMap<price, FIFO queue>` per side, so iteration order is
//! fully determined by (price, engine-assigned order id) — no clocks, no
//! hashing order, no randomness.

use std::collections::{BTreeMap, HashMap, VecDeque};

use crate::types::{AccountId, OrderId, Side};

/// An order resting on the book.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestingOrder {
    /// Engine-assigned id (also the time-priority key).
    pub id: OrderId,
    /// Owning account.
    pub account: AccountId,
    /// Remaining unfilled quantity in lots.
    pub qty: u64,
}

/// One side plus the other side of a single market's book.
#[derive(Debug, Default)]
pub struct OrderBook {
    bids: BTreeMap<u64, VecDeque<RestingOrder>>,
    asks: BTreeMap<u64, VecDeque<RestingOrder>>,
    /// Cancel index: order id → (side, price level).
    index: HashMap<OrderId, (Side, u64)>,
}

impl OrderBook {
    /// An empty book.
    pub fn new() -> Self {
        Self::default()
    }

    /// Best bid price, if any.
    pub fn best_bid(&self) -> Option<u64> {
        self.bids.keys().next_back().copied()
    }

    /// Best ask price, if any.
    pub fn best_ask(&self) -> Option<u64> {
        self.asks.keys().next().copied()
    }

    /// Number of resting orders across both sides.
    pub fn depth(&self) -> usize {
        self.index.len()
    }

    /// Whether an incoming order at `price` on `side` would cross the book.
    pub fn would_cross(&self, side: Side, price: u64) -> bool {
        match side {
            Side::Bid => self.best_ask().is_some_and(|a| price >= a),
            Side::Ask => self.best_bid().is_some_and(|b| price <= b),
        }
    }

    /// Rest an order on the book at `price`.
    pub fn insert(&mut self, side: Side, price: u64, order: RestingOrder) {
        self.index.insert(order.id, (side, price));
        self.levels_mut(side)
            .entry(price)
            .or_default()
            .push_back(order);
    }

    /// Remove a resting order by id. Returns it if it was resting.
    pub fn remove(&mut self, id: OrderId) -> Option<RestingOrder> {
        let (side, price) = self.index.remove(&id)?;
        let levels = self.levels_mut(side);
        let queue = levels.get_mut(&price)?;
        let pos = queue.iter().position(|o| o.id == id)?;
        let order = queue.remove(pos);
        if queue.is_empty() {
            levels.remove(&price);
        }
        order
    }

    /// Owner of a resting order, if it is resting.
    pub fn owner_of(&self, id: OrderId) -> Option<AccountId> {
        let (side, price) = *self.index.get(&id)?;
        self.levels(side)
            .get(&price)?
            .iter()
            .find(|o| o.id == id)
            .map(|o| o.account)
    }

    /// Peek the front-of-queue order at the best crossing level against an
    /// incoming order on `taker_side` limited to `limit_price`.
    pub fn best_counter(&self, taker_side: Side, limit_price: u64) -> Option<(u64, RestingOrder)> {
        match taker_side {
            Side::Bid => {
                let (price, queue) = self.asks.iter().next()?;
                if *price > limit_price {
                    return None;
                }
                Some((*price, *queue.front()?))
            }
            Side::Ask => {
                let (price, queue) = self.bids.iter().next_back()?;
                if *price < limit_price {
                    return None;
                }
                Some((*price, *queue.front()?))
            }
        }
    }

    /// Reduce the front-of-queue order at (`side`, `price`) by `qty`,
    /// removing it when fully consumed.
    pub fn consume_front(&mut self, side: Side, price: u64, qty: u64) {
        let mut removed_id = None;
        {
            let levels = match side {
                Side::Bid => &mut self.bids,
                Side::Ask => &mut self.asks,
            };
            let Some(queue) = levels.get_mut(&price) else {
                return;
            };
            let Some(front) = queue.front_mut() else {
                return;
            };
            if front.qty > qty {
                front.qty -= qty;
            } else {
                removed_id = Some(front.id);
                queue.pop_front();
                if queue.is_empty() {
                    levels.remove(&price);
                }
            }
        }
        if let Some(id) = removed_id {
            self.index.remove(&id);
        }
    }

    /// Total resting quantity available to an incoming order on `taker_side`
    /// at prices no worse than `limit_price`, excluding orders owned by
    /// `exclude` (self-trade-aware fill-or-kill pre-check).
    pub fn crossable_qty(&self, taker_side: Side, limit_price: u64, exclude: AccountId) -> u128 {
        let sum = |orders: &VecDeque<RestingOrder>| {
            orders
                .iter()
                .filter(|o| o.account != exclude)
                .map(|o| u128::from(o.qty))
                .sum::<u128>()
        };
        match taker_side {
            Side::Bid => self.asks.range(..=limit_price).map(|(_, q)| sum(q)).sum(),
            Side::Ask => self.bids.range(limit_price..).map(|(_, q)| sum(q)).sum(),
        }
    }

    fn levels(&self, side: Side) -> &BTreeMap<u64, VecDeque<RestingOrder>> {
        match side {
            Side::Bid => &self.bids,
            Side::Ask => &self.asks,
        }
    }

    fn levels_mut(&mut self, side: Side) -> &mut BTreeMap<u64, VecDeque<RestingOrder>> {
        match side {
            Side::Bid => &mut self.bids,
            Side::Ask => &mut self.asks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acct(b: u8) -> AccountId {
        AccountId([b; 32])
    }

    #[test]
    fn price_time_priority_and_cancel() {
        let mut book = OrderBook::new();
        book.insert(
            Side::Ask,
            105,
            RestingOrder {
                id: OrderId(1),
                account: acct(1),
                qty: 5,
            },
        );
        book.insert(
            Side::Ask,
            103,
            RestingOrder {
                id: OrderId(2),
                account: acct(2),
                qty: 7,
            },
        );
        book.insert(
            Side::Ask,
            103,
            RestingOrder {
                id: OrderId(3),
                account: acct(3),
                qty: 9,
            },
        );
        assert_eq!(book.best_ask(), Some(103));
        // Best counter for a bid at 104: price 103, earliest order (id 2).
        let (price, front) = book.best_counter(Side::Bid, 104).unwrap();
        assert_eq!((price, front.id), (103, OrderId(2)));
        // 105 level is not crossable at limit 104.
        assert_eq!(book.crossable_qty(Side::Bid, 104, acct(9)), 16);
        // Cancel the front order; priority moves to id 3.
        assert_eq!(book.remove(OrderId(2)).unwrap().qty, 7);
        let (_, front) = book.best_counter(Side::Bid, 104).unwrap();
        assert_eq!(front.id, OrderId(3));
        assert_eq!(book.depth(), 2);
    }

    #[test]
    fn consume_front_partial_and_full() {
        let mut book = OrderBook::new();
        book.insert(
            Side::Bid,
            99,
            RestingOrder {
                id: OrderId(4),
                account: acct(4),
                qty: 10,
            },
        );
        book.consume_front(Side::Bid, 99, 4);
        let (_, front) = book.best_counter(Side::Ask, 90).unwrap();
        assert_eq!(front.qty, 6);
        book.consume_front(Side::Bid, 99, 6);
        assert!(book.best_bid().is_none());
        assert_eq!(book.depth(), 0);
    }
}
