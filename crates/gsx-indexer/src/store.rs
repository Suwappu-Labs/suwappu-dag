//! Indexer storage layer.
//!
//! The MVP keeps everything in memory behind a `RwLock`. The trait
//! shape lets the Postgres adapter (T7 follow-up) slot in without
//! changing call sites in [`crate::subscriber`] or [`crate::api`].

use std::{collections::BTreeMap, sync::Arc};

use gsx_rpc::context::EventView;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// One committed block as seen by the indexer.
///
/// Built from a `"committed"` event plus the carried `intent_hashes`.
/// Richer fields (full block content, balances) come from a
/// follow-up `gsx_getBlock` backfill query in T7.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexedBlock {
    /// Consensus round at which this block was committed.
    pub round: u64,
    /// Wall-clock unix-ms at the moment the indexer ingested the
    /// `committed` event.
    pub indexed_at_ms: u64,
    /// 32-byte cert hash, hex-encoded with `0x` prefix.
    pub cert_hash: String,
    /// Intent (transaction) hashes carried by this block, in
    /// commit order. Hex with `0x` prefix.
    pub tx_hashes: Vec<String>,
}

/// Pluggable indexer store trait.
///
/// `Send + Sync + 'static` so the axum HTTP layer can hold an
/// `Arc<S: Store>` across request tasks. Uses Rust 1.75+
/// async-fn-in-trait; **not dyn-compatible** — consumers
/// parameterize over `S: Store` (same pattern as `gsx_rpc::StateView`).
pub trait Store: Send + Sync + 'static {
    /// Append a newly-observed committed block. Idempotent — if the
    /// same `round` is ingested twice (catch-up backfill overlap
    /// with the live stream), the second call is a no-op.
    fn ingest_committed_block(
        &self,
        block: IndexedBlock,
    ) -> impl std::future::Future<Output = ()> + Send;

    /// Highest committed round known to the indexer, or `None` if
    /// the store is empty. Used by T7's catch-up to find the gap
    /// boundary.
    fn latest_round(&self) -> impl std::future::Future<Output = Option<u64>> + Send;

    /// Fetch a single committed block by round.
    fn get_block(
        &self,
        round: u64,
    ) -> impl std::future::Future<Output = Option<IndexedBlock>> + Send;

    /// Inclusive range query `[from, to]`. Caller is responsible for
    /// keeping the range reasonable; the MVP returns up to 1024
    /// blocks per call.
    fn get_blocks(
        &self,
        from: u64,
        to: u64,
    ) -> impl std::future::Future<Output = Vec<IndexedBlock>> + Send;
}

/// In-memory store. Used by the MVP scaffold. Cheap to clone (one
/// `Arc<RwLock<…>>` field).
#[derive(Debug, Clone, Default)]
pub struct InMemoryStore {
    inner: Arc<RwLock<InMemoryInner>>,
}

#[derive(Debug, Default)]
struct InMemoryInner {
    /// `round → block`. `BTreeMap` so range queries are cheap.
    blocks: BTreeMap<u64, IndexedBlock>,
    /// `tx_hash → round`. Fast lookup for `/tx/:hash`.
    tx_index: BTreeMap<String, u64>,
}

impl InMemoryStore {
    /// Construct an empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Store for InMemoryStore {
    async fn ingest_committed_block(&self, block: IndexedBlock) {
        let mut inner = self.inner.write().await;
        // Idempotent: skip if already present.
        if inner.blocks.contains_key(&block.round) {
            return;
        }
        for tx in &block.tx_hashes {
            inner.tx_index.insert(tx.clone(), block.round);
        }
        inner.blocks.insert(block.round, block);
    }

    async fn latest_round(&self) -> Option<u64> {
        let inner = self.inner.read().await;
        inner.blocks.keys().next_back().copied()
    }

    async fn get_block(&self, round: u64) -> Option<IndexedBlock> {
        let inner = self.inner.read().await;
        inner.blocks.get(&round).cloned()
    }

    async fn get_blocks(&self, from: u64, to: u64) -> Vec<IndexedBlock> {
        let inner = self.inner.read().await;
        // Cap at 1024 entries to bound memory pressure on the
        // explorer side. Callers paginate by re-issuing.
        inner
            .blocks
            .range(from..=to)
            .take(1024)
            .map(|(_, b)| b.clone())
            .collect()
    }
}

/// Helper: derive an `IndexedBlock` from a `"committed"` EventView.
/// Returns `None` if the event isn't a commit (missing round or
/// cert_hash, or lane != "main").
pub fn block_from_committed_event(ev: &EventView) -> Option<IndexedBlock> {
    if ev.event != "committed" || ev.lane != "main" {
        return None;
    }
    let round = ev.round?;
    let cert_hash = ev.cert_hash.clone()?;
    let tx_hashes = ev.intent_hashes.clone().unwrap_or_default();
    Some(IndexedBlock {
        round,
        indexed_at_ms: now_ms(),
        cert_hash,
        tx_hashes,
    })
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_event(round: u64, cert: &str, txs: Vec<&str>) -> EventView {
        EventView {
            t_ms: 1,
            region: "v0".into(),
            lane: "main".into(),
            event: "committed".into(),
            round: Some(round),
            cert_hash: Some(cert.into()),
            tx_hash: None,
            peer: None,
            intent_hashes: Some(txs.into_iter().map(String::from).collect()),
            authority_id: None,
            kind: None,
            received_60s: None,
        }
    }

    #[test]
    fn block_from_committed_event_extracts_fields() {
        let ev = mk_event(42, "0xab", vec!["0xcd", "0xef"]);
        let block = block_from_committed_event(&ev).unwrap();
        assert_eq!(block.round, 42);
        assert_eq!(block.cert_hash, "0xab");
        assert_eq!(
            block.tx_hashes,
            vec!["0xcd".to_string(), "0xef".to_string()]
        );
    }

    #[test]
    fn block_from_committed_event_rejects_non_committed() {
        let mut ev = mk_event(42, "0xab", vec![]);
        ev.event = "proposed".into();
        assert!(block_from_committed_event(&ev).is_none());
    }

    #[test]
    fn block_from_committed_event_rejects_off_lane() {
        let mut ev = mk_event(42, "0xab", vec![]);
        ev.lane = "fastpath".into();
        assert!(block_from_committed_event(&ev).is_none());
    }

    #[tokio::test]
    async fn ingest_is_idempotent() {
        let store = InMemoryStore::new();
        let block = IndexedBlock {
            round: 7,
            indexed_at_ms: 100,
            cert_hash: "0xab".into(),
            tx_hashes: vec!["0xcd".into()],
        };
        store.ingest_committed_block(block.clone()).await;
        store.ingest_committed_block(block.clone()).await;
        assert_eq!(store.latest_round().await, Some(7));
        assert_eq!(store.get_block(7).await, Some(block));
    }

    #[tokio::test]
    async fn range_query_is_inclusive() {
        let store = InMemoryStore::new();
        for r in 0..10u64 {
            let block = IndexedBlock {
                round: r,
                indexed_at_ms: 100,
                cert_hash: format!("0x{:02x}", r),
                tx_hashes: vec![],
            };
            store.ingest_committed_block(block).await;
        }
        let blocks = store.get_blocks(3, 6).await;
        assert_eq!(blocks.len(), 4);
        assert_eq!(blocks.first().unwrap().round, 3);
        assert_eq!(blocks.last().unwrap().round, 6);
    }

    #[tokio::test]
    async fn latest_round_empty_is_none() {
        let store = InMemoryStore::new();
        assert_eq!(store.latest_round().await, None);
    }
}
