//! The mempool itself.
//!
//! See the crate root for architecture.

use std::{
    cmp::Reverse,
    collections::{BTreeMap, HashMap, HashSet},
    sync::Mutex,
};

use gsx_execution::Intent;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::bucket::LeakyBucket;

/// Canonical content hash of an intent: `blake3(bincode(intent))`.
/// Same value the JSON-RPC `gsx_submitIntent` returns and the
/// round-driver indexes into `tx_to_block`. Hex-encoding (with `0x`
/// prefix) is the wire form; the byte form is for internal lookups.
pub type IntentHash = [u8; 32];

/// Tunable knobs for a `Mempool` instance.
#[derive(Debug, Clone)]
pub struct MempoolConfig {
    /// Maximum total entries. When full, a submission with priority
    /// higher than the current floor evicts the floor; a submission
    /// at-or-below the floor is rejected. Default: 10 000.
    pub capacity: usize,
    /// Submission expiry: an entry older than this is dropped on the
    /// next `expire` sweep. Default: 60 000 (60 s).
    pub ttl_ms: u64,
    /// Per-peer bucket capacity (burst allowance). Default: 100.
    pub per_peer_capacity: u64,
    /// Per-peer refill rate, tokens/sec. Default: 50.
    pub per_peer_refill_per_sec: u64,
}

impl Default for MempoolConfig {
    fn default() -> Self {
        Self {
            capacity: 10_000,
            ttl_ms: 60_000,
            per_peer_capacity: 100,
            per_peer_refill_per_sec: 50,
        }
    }
}

/// Surface-able snapshot of mempool internals. Cheap to compute
/// (single mutex acquisition). Use this for `/health`, metrics
/// scraping, or test assertions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MempoolStats {
    /// Current number of entries.
    pub size: usize,
    /// Configured capacity.
    pub capacity: usize,
    /// Highest priority currently held. `None` if empty.
    pub max_priority: Option<u64>,
    /// Lowest priority currently held. `None` if empty.
    pub min_priority: Option<u64>,
    /// Number of distinct peers we've ever tracked a bucket for.
    pub tracked_peers: usize,
}

#[derive(Debug, Error)]
pub enum MempoolError {
    /// Peer's leaky bucket is empty. Wait `retry_after_ms` and retry.
    #[error("rate limited: retry in {retry_after_ms} ms")]
    RateLimited { retry_after_ms: u64 },
    /// Same intent (by content hash) already queued. Not a wire-level
    /// error — clients re-submitting after a network blip will hit
    /// this. The hash returned is the existing entry.
    #[error("duplicate intent: {hash:?} already queued")]
    DuplicateIntent { hash: IntentHash },
    /// Mempool is at capacity and the submitted priority is at or
    /// below the current floor. Submitter must either raise priority
    /// or wait.
    #[error("priority {given} below floor {floor}: mempool at capacity {capacity}")]
    BelowFloor {
        given: u64,
        floor: u64,
        capacity: usize,
    },
    /// Bincode serialization failed. Shouldn't happen for well-formed
    /// `Intent` values; surfaced for completeness.
    #[error("bincode encode failed: {0}")]
    Encode(String),
}

/// Sort key used by the priority `BTreeMap`. With `Reverse(priority)`
/// in the first slot, `BTreeMap`'s natural ascending iteration order
/// becomes "highest priority first." Older `submit_ms` and lower hash
/// break ties deterministically so two daemons drain in the same
/// order.
type EntryKey = (Reverse<u64>, u64, IntentHash);

#[derive(Debug, Clone)]
struct Entry {
    intent: Intent,
    submit_ms: u64,
}

struct Inner {
    /// `EntryKey → Entry`. `first_key_value` is the highest-priority
    /// (drained first); `last_key_value` is the floor (evicted first).
    queue: BTreeMap<EntryKey, Entry>,
    /// Content-hash dedup set. Mirrors `queue.keys().map(|k| k.2)`.
    seen: HashSet<IntentHash>,
    /// One bucket per peer label. Peers with no submissions in
    /// memory still consume one bucket entry until the next
    /// `gc_buckets` call.
    buckets: HashMap<String, LeakyBucket>,
}

/// Bounded priority + rate-limited mempool. Cheap to clone the
/// `Arc<Mempool>` the daemon shares between client ingress and the
/// round-driver.
pub struct Mempool {
    inner: Mutex<Inner>,
    config: MempoolConfig,
}

impl Mempool {
    /// Construct an empty mempool with the given config.
    pub fn new(config: MempoolConfig) -> Self {
        Self {
            inner: Mutex::new(Inner {
                queue: BTreeMap::new(),
                seen: HashSet::new(),
                buckets: HashMap::new(),
            }),
            config,
        }
    }

    /// Submit a verified intent. `priority` is opaque to the mempool;
    /// higher values drain first. `peer` is the rate-limit bucket key
    /// (typically the wire-level remote address or pubkey hash); pass
    /// `None` for trusted submissions (e.g. operator-driven cleanup
    /// intents) that should bypass per-peer limits.
    ///
    /// Caller MUST have already verified the intent's signature.
    /// The mempool does not re-verify.
    pub fn submit(
        &self,
        intent: Intent,
        priority: u64,
        peer: Option<String>,
        now_ms: u64,
    ) -> Result<IntentHash, MempoolError> {
        // 1. Canonical hash up front, before taking the lock.
        let bytes = bincode::serialize(&intent).map_err(|e| MempoolError::Encode(e.to_string()))?;
        let hash: IntentHash = *blake3::hash(&bytes).as_bytes();

        let mut inner = self.inner.lock().unwrap();

        // 2. Rate limit (only if peer label provided).
        if let Some(ref label) = peer {
            let bucket = inner.buckets.entry(label.clone()).or_insert_with(|| {
                LeakyBucket::new(
                    self.config.per_peer_capacity,
                    self.config.per_peer_refill_per_sec,
                    now_ms,
                )
            });
            if let Err(retry_after_ms) = bucket.take_one(now_ms) {
                return Err(MempoolError::RateLimited { retry_after_ms });
            }
        }

        // 3. Dedup: same content already queued?
        if inner.seen.contains(&hash) {
            return Err(MempoolError::DuplicateIntent { hash });
        }

        // 4. Capacity check + eviction.
        if inner.queue.len() >= self.config.capacity {
            // Floor = (Reverse(min_priority), max_submit_ms, max_hash).
            // BTreeMap's last entry is the lexicographic max of the key,
            // which with Reverse(priority) on top is the entry with the
            // LOWEST priority (and latest submit_ms / largest hash on
            // tie — i.e. the cheapest-most-recent submission).
            let floor_priority = inner.queue.keys().next_back().map(|k| k.0 .0).unwrap_or(0);
            if priority <= floor_priority {
                return Err(MempoolError::BelowFloor {
                    given: priority,
                    floor: floor_priority,
                    capacity: self.config.capacity,
                });
            }
            // Evict the floor entry.
            if let Some((evicted_key, _)) = inner.queue.pop_last() {
                inner.seen.remove(&evicted_key.2);
            }
        }

        // 5. Insert. `peer` was used for rate-limit accounting above
        // and is intentionally dropped here — the mempool itself
        // doesn't carry submitter identity into the block.
        let _ = peer;
        let key: EntryKey = (Reverse(priority), now_ms, hash);
        inner.seen.insert(hash);
        inner.queue.insert(
            key,
            Entry {
                intent,
                submit_ms: now_ms,
            },
        );
        Ok(hash)
    }

    /// Pop up to `max` highest-priority intents for inclusion in a
    /// block. Drained entries are removed from the mempool; the
    /// daemon owns them from that point on.
    pub fn drain_for_block(&self, max: usize) -> Vec<Intent> {
        let mut inner = self.inner.lock().unwrap();
        let mut out = Vec::with_capacity(max.min(inner.queue.len()));
        while out.len() < max {
            // pop_first is O(log n) and yields the HIGHEST priority
            // because of the Reverse() in the key tuple.
            match inner.queue.pop_first() {
                Some((key, entry)) => {
                    inner.seen.remove(&key.2);
                    out.push(entry.intent);
                }
                None => break,
            }
        }
        out
    }

    /// Drop entries older than `config.ttl_ms`. Called periodically by
    /// the daemon. Returns the number of entries dropped (handy for
    /// metrics and tests).
    pub fn expire(&self, now_ms: u64) -> usize {
        let mut inner = self.inner.lock().unwrap();
        let ttl = self.config.ttl_ms;
        let mut to_drop: Vec<EntryKey> = Vec::new();
        for (key, entry) in inner.queue.iter() {
            if now_ms.saturating_sub(entry.submit_ms) > ttl {
                to_drop.push(*key);
            }
        }
        for key in &to_drop {
            inner.queue.remove(key);
            inner.seen.remove(&key.2);
        }
        to_drop.len()
    }

    /// Snapshot the current state. Cheap; takes the mutex once.
    pub fn stats(&self) -> MempoolStats {
        let inner = self.inner.lock().unwrap();
        let max_priority = inner.queue.keys().next().map(|k| k.0 .0);
        let min_priority = inner.queue.keys().next_back().map(|k| k.0 .0);
        MempoolStats {
            size: inner.queue.len(),
            capacity: self.config.capacity,
            max_priority,
            min_priority,
            tracked_peers: inner.buckets.len(),
        }
    }

    /// Drop tracked bucket entries that have been idle (full + unused)
    /// since `idle_since_ms`. Lets a long-running daemon shed memory
    /// for one-shot submitters without affecting active peers.
    pub fn gc_buckets(&self, now_ms: u64, idle_threshold_ms: u64) -> usize {
        let mut inner = self.inner.lock().unwrap();
        let before = inner.buckets.len();
        inner.buckets.retain(|_, bucket| {
            // We treat a bucket as "active" if it was touched within
            // the threshold. `last_refill_ms` is updated on every
            // `take_one` call, so this is a proxy for activity.
            let last = bucket_last_activity(bucket);
            now_ms.saturating_sub(last) < idle_threshold_ms
        });
        before - inner.buckets.len()
    }
}

/// Helper: extract `last_refill_ms` from a `LeakyBucket` without
/// adding a public accessor (keeps the bucket API minimal).
fn bucket_last_activity(bucket: &LeakyBucket) -> u64 {
    // `LeakyBucket` is Copy + small. We can serialize-and-extract,
    // but the simpler path is a single accessor; expose one.
    bucket.last_refill_ms_for_gc()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(byte: u8) -> [u8; 20] {
        [byte; 20]
    }

    fn transfer(amount: u128) -> Intent {
        Intent::Transfer {
            from: addr(1),
            to: addr(2),
            amount,
        }
    }

    fn mp(capacity: usize) -> Mempool {
        Mempool::new(MempoolConfig {
            capacity,
            ttl_ms: 60_000,
            // Per-peer limit lifted in these tests; they're exercised
            // separately in the peer-limit case.
            per_peer_capacity: u64::MAX,
            per_peer_refill_per_sec: u64::MAX,
        })
    }

    #[test]
    fn submit_then_drain_yields_highest_priority_first() {
        let mp = mp(10);
        mp.submit(transfer(1), 10, None, 100).unwrap();
        mp.submit(transfer(2), 30, None, 101).unwrap();
        mp.submit(transfer(3), 20, None, 102).unwrap();
        let drained = mp.drain_for_block(3);
        assert_eq!(drained.len(), 3);
        // priority 30 → 20 → 10
        assert!(matches!(drained[0], Intent::Transfer { amount: 2, .. }));
        assert!(matches!(drained[1], Intent::Transfer { amount: 3, .. }));
        assert!(matches!(drained[2], Intent::Transfer { amount: 1, .. }));
    }

    #[test]
    fn submit_dedups_by_content_hash() {
        let mp = mp(10);
        mp.submit(transfer(7), 5, None, 100).unwrap();
        let err = mp.submit(transfer(7), 5, None, 101).unwrap_err();
        assert!(matches!(err, MempoolError::DuplicateIntent { .. }));
    }

    #[test]
    fn ties_break_by_submit_ms_then_hash() {
        let mp = mp(10);
        // Same priority. Older entry should drain first.
        let h_a = mp.submit(transfer(100), 5, None, 100).unwrap();
        let h_b = mp.submit(transfer(200), 5, None, 200).unwrap();
        assert_ne!(h_a, h_b);
        let drained = mp.drain_for_block(2);
        assert!(
            matches!(drained[0], Intent::Transfer { amount: 100, .. }),
            "older submit_ms should drain first"
        );
    }

    #[test]
    fn capacity_evicts_floor_when_higher_priority_arrives() {
        let mp = mp(3);
        mp.submit(transfer(1), 10, None, 0).unwrap();
        mp.submit(transfer(2), 20, None, 0).unwrap();
        mp.submit(transfer(3), 30, None, 0).unwrap();
        assert_eq!(mp.stats().size, 3);
        // Priority 40 evicts the floor (priority 10 entry).
        mp.submit(transfer(4), 40, None, 0).unwrap();
        assert_eq!(mp.stats().size, 3);
        let drained = mp.drain_for_block(3);
        // The 10-priority intent (amount=1) was evicted.
        assert!(drained
            .iter()
            .all(|i| !matches!(i, Intent::Transfer { amount: 1, .. })));
    }

    #[test]
    fn capacity_rejects_below_floor() {
        let mp = mp(2);
        mp.submit(transfer(1), 100, None, 0).unwrap();
        mp.submit(transfer(2), 200, None, 0).unwrap();
        let err = mp.submit(transfer(3), 50, None, 0).unwrap_err();
        match err {
            MempoolError::BelowFloor {
                given,
                floor,
                capacity,
            } => {
                assert_eq!(given, 50);
                assert_eq!(floor, 100);
                assert_eq!(capacity, 2);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn rate_limit_per_peer_kicks_in_after_burst() {
        let mp = Mempool::new(MempoolConfig {
            capacity: 100,
            ttl_ms: 60_000,
            per_peer_capacity: 3,
            per_peer_refill_per_sec: 0,
        });
        for n in 0u128..3 {
            mp.submit(transfer(n), 1, Some("peerA".into()), 0).unwrap();
        }
        let err = mp
            .submit(transfer(99), 1, Some("peerA".into()), 0)
            .unwrap_err();
        assert!(matches!(err, MempoolError::RateLimited { .. }));
        // Different peer is unaffected.
        mp.submit(transfer(1000), 1, Some("peerB".into()), 0)
            .unwrap();
    }

    #[test]
    fn expire_drops_entries_past_ttl() {
        let mp = Mempool::new(MempoolConfig {
            capacity: 100,
            ttl_ms: 1_000,
            per_peer_capacity: u64::MAX,
            per_peer_refill_per_sec: u64::MAX,
        });
        mp.submit(transfer(1), 5, None, 0).unwrap();
        mp.submit(transfer(2), 5, None, 500).unwrap();
        mp.submit(transfer(3), 5, None, 2_000).unwrap();
        // At t=2_500: entry-0 is 2 500 ms old (>1 s), entry-1 is 2 000 ms
        // old (>1 s), entry-2 is 500 ms old (<1 s).
        let dropped = mp.expire(2_500);
        assert_eq!(dropped, 2);
        assert_eq!(mp.stats().size, 1);
        let drained = mp.drain_for_block(10);
        assert_eq!(drained.len(), 1);
        assert!(matches!(drained[0], Intent::Transfer { amount: 3, .. }));
    }

    #[test]
    fn stats_reflect_current_state() {
        let mp = mp(10);
        assert_eq!(mp.stats().size, 0);
        assert!(mp.stats().max_priority.is_none());
        mp.submit(transfer(1), 100, None, 0).unwrap();
        mp.submit(transfer(2), 50, None, 1).unwrap();
        let s = mp.stats();
        assert_eq!(s.size, 2);
        assert_eq!(s.max_priority, Some(100));
        assert_eq!(s.min_priority, Some(50));
    }

    #[test]
    fn gc_buckets_drops_idle_only() {
        let mp = Mempool::new(MempoolConfig {
            capacity: 100,
            ttl_ms: 60_000,
            per_peer_capacity: 10,
            per_peer_refill_per_sec: 10,
        });
        mp.submit(transfer(1), 1, Some("active".into()), 0).unwrap();
        mp.submit(transfer(2), 1, Some("idle".into()), 0).unwrap();
        // Activity on "active" at t=10_000.
        mp.submit(transfer(3), 1, Some("active".into()), 10_000)
            .unwrap();
        // GC at t=20_000 with threshold 15_000ms (15s):
        //   - "active" last touched at 10_000 → 10s old, < 15s, keep
        //   - "idle"   last touched at      0 → 20s old, > 15s, drop
        let dropped = mp.gc_buckets(20_000, 15_000);
        assert_eq!(dropped, 1);
        assert_eq!(mp.stats().tracked_peers, 1);
    }

    #[test]
    fn drain_for_block_caps_at_max() {
        let mp = mp(10);
        for n in 0u128..5 {
            mp.submit(transfer(n), 1, None, n as u64).unwrap();
        }
        let drained = mp.drain_for_block(2);
        assert_eq!(drained.len(), 2);
        assert_eq!(mp.stats().size, 3);
    }

    #[test]
    fn drain_for_block_empty_returns_empty() {
        let mp = mp(10);
        let drained = mp.drain_for_block(100);
        assert!(drained.is_empty());
    }

    #[test]
    fn submit_with_no_peer_bypasses_rate_limit() {
        let mp = Mempool::new(MempoolConfig {
            capacity: 100,
            ttl_ms: 60_000,
            per_peer_capacity: 1,
            per_peer_refill_per_sec: 0,
        });
        // Many unattributed submissions: no bucket consumed.
        for n in 0u128..100 {
            mp.submit(transfer(n), 1, None, 0).unwrap();
        }
        assert_eq!(mp.stats().size, 100);
    }
}
