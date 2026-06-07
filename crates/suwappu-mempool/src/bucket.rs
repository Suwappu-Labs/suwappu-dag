//! Per-peer leaky-bucket rate limiter.
//!
//! Implements a classic token bucket: each peer has `capacity` tokens
//! that refill at a configured rate. Every successful submission costs
//! one token. When the bucket is empty, further submissions are
//! rejected with the suggested retry-after delay.
//!
//! Construction: `new(capacity, refill_per_sec, now_ms)` for per-second
//! rates, or `from_per_hour(capacity, refill_per_hour, now_ms)` for
//! sub-per-second rates (avoids the integer-rounding trap of dividing
//! a small per-hour value by 3600).
//!
//! This is the same algorithm used by every cloud provider's rate
//! limiter (AWS, GCP, Cloudflare) because it (a) absorbs bursts up to
//! capacity, (b) imposes a steady-state ceiling, and (c) has O(1)
//! memory per peer.

/// A single peer's bucket state.
#[derive(Debug, Clone, Copy)]
pub struct LeakyBucket {
    /// Tokens currently available. Saturates at `capacity`.
    tokens: f64,
    /// Bucket size — maximum burst.
    capacity: f64,
    /// Refill rate in tokens-per-millisecond.
    refill_per_ms: f64,
    /// Wall-clock unix millis at which `tokens` was last refreshed.
    last_refill_ms: u64,
}

impl LeakyBucket {
    /// Construct a full bucket. The peer starts with a full burst
    /// allowance.
    pub fn new(capacity: u64, refill_per_sec: u64, now_ms: u64) -> Self {
        Self {
            tokens: capacity as f64,
            capacity: capacity as f64,
            refill_per_ms: refill_per_sec as f64 / 1000.0,
            last_refill_ms: now_ms,
        }
    }

    /// Construct a bucket using a per-hour refill rate. Avoids the
    /// integer-rounding trap in `new()` where sub-1-per-second rates
    /// (e.g. 5/hour) get `ceil()`-ed to 1/sec (H19).
    pub fn from_per_hour(capacity: u64, refill_per_hour: u64, now_ms: u64) -> Self {
        Self {
            tokens: capacity as f64,
            capacity: capacity as f64,
            refill_per_ms: refill_per_hour as f64 / 3_600_000.0,
            last_refill_ms: now_ms,
        }
    }

    /// Refill the bucket up to capacity based on elapsed time.
    fn refill(&mut self, now_ms: u64) {
        let elapsed = now_ms.saturating_sub(self.last_refill_ms) as f64;
        if elapsed > 0.0 {
            self.tokens = (self.tokens + elapsed * self.refill_per_ms).min(self.capacity);
            self.last_refill_ms = now_ms;
        }
    }

    /// Wall-clock millis at which this bucket was last touched.
    /// Used by `Mempool::gc_buckets` to age out idle peers.
    pub fn last_refill_ms_for_gc(&self) -> u64 {
        self.last_refill_ms
    }

    /// Try to consume one token. Returns `Ok(())` on success, or
    /// `Err(retry_after_ms)` indicating when the next token will be
    /// available.
    pub fn take_one(&mut self, now_ms: u64) -> Result<(), u64> {
        self.refill(now_ms);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            Ok(())
        } else {
            // Tokens needed to reach 1.0, in milliseconds. If
            // `refill_per_ms` is 0, the division yields +inf and the
            // saturating cast lands at `u64::MAX` — the bucket is
            // permanently empty, which is exactly what we want.
            let needed = 1.0 - self.tokens;
            Err((needed / self.refill_per_ms).ceil() as u64)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_bucket_admits_burst_up_to_capacity() {
        let mut b = LeakyBucket::new(5, 1, 1_000);
        for _ in 0..5 {
            assert!(b.take_one(1_000).is_ok());
        }
        let retry = b.take_one(1_000).unwrap_err();
        assert!(retry >= 1_000, "expected ≥1s wait, got {}ms", retry);
    }

    #[test]
    fn refills_at_configured_rate() {
        let mut b = LeakyBucket::new(2, 10, 0);
        b.take_one(0).unwrap();
        b.take_one(0).unwrap();
        // 100ms at 10 tokens/sec = 1 token.
        assert!(b.take_one(100).is_ok());
        // Immediate retry — bucket empty again.
        assert!(b.take_one(100).is_err());
    }

    #[test]
    fn refill_saturates_at_capacity() {
        let mut b = LeakyBucket::new(3, 100, 0);
        // Long idle: bucket would have refilled 100k tokens if uncapped.
        // Should saturate at 3.
        for _ in 0..3 {
            assert!(b.take_one(1_000_000).is_ok());
        }
        assert!(b.take_one(1_000_000).is_err());
    }

    #[test]
    fn zero_refill_rate_means_permanent_lockout_after_burst() {
        let mut b = LeakyBucket::new(1, 0, 0);
        b.take_one(0).unwrap();
        // After consuming the only token, no refill ever happens.
        let retry = b.take_one(u64::MAX / 2).unwrap_err();
        assert_eq!(retry, u64::MAX);
    }

    #[test]
    fn from_per_hour_refills_correctly() {
        // 3600 tokens/hour = 1 token/sec = 1 token per 1000ms.
        let mut b = LeakyBucket::from_per_hour(1, 3600, 0);
        b.take_one(0).unwrap();
        // After 999ms: not yet refilled.
        assert!(b.take_one(999).is_err());
        // After 1000ms from start: 1 token refilled.
        assert!(b.take_one(1000).is_ok());
    }

    #[test]
    fn from_per_hour_handles_sub_per_second_rates() {
        // 5 tokens/hour — the faucet's default. Must NOT round to
        // 1/sec (the H19 bug). 5/hour = 1 token per 720 seconds.
        let mut b = LeakyBucket::from_per_hour(1, 5, 0);
        b.take_one(0).unwrap();
        // After 719 seconds: not yet refilled.
        assert!(b.take_one(719_000).is_err());
        // After 720 seconds: 1 token refilled.
        assert!(b.take_one(720_000).is_ok());
    }
}
