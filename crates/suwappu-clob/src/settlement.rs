//! Clearing and settlement: multilateral netting of fills into per-account
//! deltas and a constant-size batch commitment.
//!
//! A settlement batch nets every fill in a matching window into one signed
//! (base, quote) delta per account, then commits the whole batch as a single
//! domain-separated SHA3-256 root. The root — not the fills — is what the
//! execution substrate applies against the balance map and what the LTP
//! anchor pipeline attests cross-chain, keeping the on-chain commitment
//! surface constant-size regardless of trade count.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use suwappu_crypto::hash::sha3_256_domain;

use crate::types::{AccountId, ClobError, Fill, MarketId, Side};

/// Domain separation tag for settlement batch roots. Byte-identical across
/// every implementation that verifies these commitments.
pub const SETTLEMENT_DST: &[u8] = b"SUWAPPU-CLOB-SETTLEMENT-V1";

/// Net position change for one account within a batch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountDelta {
    /// Net base-asset change in lots (positive = received base).
    pub base: i128,
    /// Net quote-asset change in ticks×lots (positive = received quote).
    pub quote: i128,
}

/// A cleared batch of fills for one market: per-account net deltas plus the
/// fill-count and fill-sequence window they cover.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlementBatch {
    /// Market the batch settles.
    pub market: MarketId,
    /// Net deltas, keyed by account. `BTreeMap` fixes the encoding order.
    pub deltas: BTreeMap<AccountId, AccountDelta>,
    /// Number of fills netted into the batch.
    pub fill_count: u64,
    /// First fill sequence number in the window (0 when empty).
    pub first_seq: u64,
    /// Last fill sequence number in the window (0 when empty).
    pub last_seq: u64,
}

impl SettlementBatch {
    /// Net `fills` into a settlement batch for `market`.
    ///
    /// Fails on notional overflow rather than wrapping — a batch that cannot
    /// be represented is a batch that must not settle.
    pub fn from_fills(market: MarketId, fills: &[Fill]) -> Result<Self, ClobError> {
        let mut deltas: BTreeMap<AccountId, AccountDelta> = BTreeMap::new();
        let mut first_seq = 0u64;
        let mut last_seq = 0u64;
        for (i, fill) in fills.iter().enumerate() {
            if i == 0 {
                first_seq = fill.seq;
            }
            last_seq = fill.seq;
            let base = i128::from(fill.qty);
            let notional = u128::from(fill.qty)
                .checked_mul(u128::from(fill.price))
                .ok_or(ClobError::NotionalOverflow)?;
            let quote = i128::try_from(notional).map_err(|_| ClobError::NotionalOverflow)?;
            let (buyer, seller) = match fill.taker_side {
                Side::Bid => (fill.taker_account, fill.maker_account),
                Side::Ask => (fill.maker_account, fill.taker_account),
            };
            let b = deltas.entry(buyer).or_default();
            b.base = b
                .base
                .checked_add(base)
                .ok_or(ClobError::NotionalOverflow)?;
            b.quote = b
                .quote
                .checked_sub(quote)
                .ok_or(ClobError::NotionalOverflow)?;
            let s = deltas.entry(seller).or_default();
            s.base = s
                .base
                .checked_sub(base)
                .ok_or(ClobError::NotionalOverflow)?;
            s.quote = s
                .quote
                .checked_add(quote)
                .ok_or(ClobError::NotionalOverflow)?;
        }
        Ok(Self {
            market,
            deltas,
            fill_count: fills.len() as u64,
            first_seq,
            last_seq,
        })
    }

    /// Whether the batch conserves value: base and quote deltas each sum to
    /// zero across all accounts. Holds by construction; exposed so callers
    /// (and invariant tests) can assert it independently.
    pub fn is_conserving(&self) -> bool {
        let (mut base, mut quote) = (0i128, 0i128);
        for d in self.deltas.values() {
            base += d.base;
            quote += d.quote;
        }
        base == 0 && quote == 0
    }

    /// Constant-size (32-byte) commitment to the batch: domain-separated
    /// SHA3-256 over the canonical big-endian encoding of the market, the
    /// fill window, and every (account, delta) pair in `BTreeMap` order.
    pub fn batch_root(&self) -> [u8; 32] {
        let mut bytes = Vec::with_capacity(32 + 24 + self.deltas.len() * 64);
        bytes.extend_from_slice(&self.market.0);
        bytes.extend_from_slice(&self.fill_count.to_be_bytes());
        bytes.extend_from_slice(&self.first_seq.to_be_bytes());
        bytes.extend_from_slice(&self.last_seq.to_be_bytes());
        for (account, delta) in &self.deltas {
            bytes.extend_from_slice(&account.0);
            bytes.extend_from_slice(&delta.base.to_be_bytes());
            bytes.extend_from_slice(&delta.quote.to_be_bytes());
        }
        sha3_256_domain(SETTLEMENT_DST, &bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::OrderId;

    fn acct(b: u8) -> AccountId {
        AccountId([b; 32])
    }

    fn fill(maker: u8, taker: u8, price: u64, qty: u64, taker_side: Side, seq: u64) -> Fill {
        Fill {
            maker_order: OrderId(seq),
            taker_order: OrderId(1000 + seq),
            maker_account: acct(maker),
            taker_account: acct(taker),
            price,
            qty,
            taker_side,
            seq,
        }
    }

    #[test]
    fn netting_conserves_and_nets_across_fills() {
        let fills = [
            fill(1, 2, 100, 5, Side::Bid, 0), // acct2 buys 5 @ 100 from acct1
            fill(2, 1, 110, 3, Side::Bid, 1), // acct1 buys 3 @ 110 from acct2
        ];
        let batch = SettlementBatch::from_fills(MarketId([7; 32]), &fills).unwrap();
        assert!(batch.is_conserving());
        let d1 = batch.deltas[&acct(1)];
        let d2 = batch.deltas[&acct(2)];
        assert_eq!(d1.base, -2); // sold 5, bought 3
        assert_eq!(d1.quote, 500 - 330);
        assert_eq!(d2.base, 2);
        assert_eq!(d2.quote, 330 - 500);
        assert_eq!(
            (batch.first_seq, batch.last_seq, batch.fill_count),
            (0, 1, 2)
        );
    }

    #[test]
    fn batch_root_is_stable_and_input_sensitive() {
        let fills = [fill(1, 2, 100, 5, Side::Bid, 0)];
        let market = MarketId([7; 32]);
        let a = SettlementBatch::from_fills(market, &fills).unwrap();
        let b = SettlementBatch::from_fills(market, &fills).unwrap();
        assert_eq!(a.batch_root(), b.batch_root());
        let other =
            SettlementBatch::from_fills(market, &[fill(1, 2, 100, 6, Side::Bid, 0)]).unwrap();
        assert_ne!(a.batch_root(), other.batch_root());
        assert_eq!(a.batch_root().len(), 32);
    }

    #[test]
    fn notional_overflow_is_rejected() {
        // u64::MAX × u64::MAX exceeds i128::MAX, so the signed accumulator
        // must refuse it rather than wrap.
        let fills = [fill(1, 2, u64::MAX, u64::MAX, Side::Bid, 0)];
        assert_eq!(
            SettlementBatch::from_fills(MarketId([7; 32]), &fills).unwrap_err(),
            ClobError::NotionalOverflow
        );

        // A large-but-representable notional accumulated twice in the same
        // direction overflows the per-account i128 delta.
        let half = 1u64 << 63;
        let twice = [
            fill(1, 2, half, half, Side::Bid, 0),
            fill(1, 2, half, half, Side::Bid, 1),
        ];
        assert_eq!(
            SettlementBatch::from_fills(MarketId([7; 32]), &twice).unwrap_err(),
            ClobError::NotionalOverflow
        );
    }
}
