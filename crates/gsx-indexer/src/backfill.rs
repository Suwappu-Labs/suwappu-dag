//! F2 (A5 Phase 2): startup catch-up backfill.
//!
//! Before the live WebSocket subscriber takes over, the indexer
//! reconciles its `Store` with the chain head by:
//!
//! 1. Asking `gsx_getEpoch` for `latest_committed_round`.
//! 2. Reading `Store::latest_round()`.
//! 3. Walking `(latest_round+1, head]` via `gsx_getBlock(round)`.
//! 4. Calling `Store::ingest_committed_block` for each (idempotency
//!    is inherited from the store — `PostgresStore` uses
//!    `INSERT … ON CONFLICT DO NOTHING`, `InMemoryStore` checks for
//!    presence before insert).
//!
//! Rounds that come back `None` from `get_block` are legitimate gaps
//! (Mysticeti-C `Skip` outcome) — they are not errors, just elided
//! from the store.
//!
//! The walk is bounded: each iteration covers `max_per_iter` rounds,
//! yielding to the runtime between batches so a giant gap doesn't
//! monopolize the startup tokio task. If the chain head advances
//! while we backfill, we don't chase the new tip in this pass — the
//! live subscriber covers everything beyond the snapshot, with the
//! second-pass idempotency from the store filling any short
//! overlap.

use anyhow::Context;
use gsx_client::Client;
use gsx_rpc::context::BlockView;
use tracing::{debug, info, warn};

use crate::store::{IndexedBlock, Store};

/// Default batch size for the catch-up walk. Each batch issues
/// `max_per_iter` `get_block` calls before yielding.
pub const DEFAULT_MAX_PER_ITER: u64 = 256;

/// Walk the gap between the store's `latest_round()` and the
/// chain head, persisting every committed block in between.
///
/// `rpc_url` must be a base HTTP URL accepted by the SDK (e.g.
/// `http://127.0.0.1:9092`). The function is one-shot — there's no
/// internal retry loop. Transient HTTP errors propagate out so the
/// caller can decide whether to abort startup or fall back to a
/// live-tail-only mode. The typical caller `main.rs` aborts.
///
/// Returns the number of blocks ingested.
pub async fn catch_up<S>(store: &S, rpc_url: &str, max_per_iter: u64) -> anyhow::Result<u64>
where
    S: Store,
{
    let max_per_iter = max_per_iter.max(1);
    let client = Client::new(rpc_url);

    let epoch = client
        .get_epoch()
        .await
        .context("backfill: get_epoch failed")?;
    let head = epoch.latest_committed_round;

    let mut next = store
        .latest_round()
        .await
        .map(|r| r.saturating_add(1))
        .unwrap_or(0);

    if next > head {
        info!(
            store_latest = next.saturating_sub(1),
            chain_head = head,
            "indexer backfill: store is at or ahead of chain head, nothing to do"
        );
        return Ok(0);
    }

    info!(
        from = next,
        chain_head = head,
        max_per_iter,
        "indexer backfill: walking gap"
    );

    let mut ingested: u64 = 0;
    while next <= head {
        let end = next
            .saturating_add(max_per_iter)
            .min(head.saturating_add(1));
        for r in next..end {
            match client.get_block(r).await {
                Ok(Some(view)) => {
                    store.ingest_committed_block(block_from_view(&view)).await;
                    ingested += 1;
                }
                Ok(None) => {
                    // Mysticeti-C `Skip` round — legitimate gap.
                    debug!(round = r, "indexer backfill: skipped round (Skip outcome)");
                }
                Err(e) => {
                    warn!(round = r, error = %e, "indexer backfill: get_block failed");
                    return Err(anyhow::anyhow!(e))
                        .with_context(|| format!("backfill: get_block(round={r}) failed"));
                }
            }
        }
        // Yield between batches so a megasized backfill doesn't
        // monopolize the runtime task.
        tokio::task::yield_now().await;
        next = end;
    }

    info!(chain_head = head, ingested, "indexer backfill: complete");
    Ok(ingested)
}

/// Project a `BlockView` (RPC shape) into the indexer's
/// `IndexedBlock`. `cert_hash` is taken verbatim; tx hashes are
/// stripped of their `0x` prefix to match the live-path storage
/// convention (the EventView path stores raw hex). `indexed_at_ms`
/// is set to "now" — the indexer's view of when it caught up, not
/// the original commit time. Live commits replace any overlapping
/// backfilled row idempotently via the store's
/// `ingest_committed_block`.
fn block_from_view(view: &BlockView) -> IndexedBlock {
    IndexedBlock {
        round: view.round,
        indexed_at_ms: now_ms(),
        cert_hash: view.cert_hash.clone(),
        tx_hashes: view
            .tx_hashes
            .iter()
            .map(|h| h.strip_prefix("0x").unwrap_or(h.as_str()).to_string())
            .collect(),
    }
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
    use crate::store::InMemoryStore;

    #[tokio::test]
    async fn block_from_view_strips_tx_hash_prefix() {
        let bv = BlockView {
            round: 7,
            cert_hash: "0xab".into(),
            intents: vec![],
            tx_hashes: vec!["0xdeadbeef".into(), "feedface".into()],
        };
        let block = block_from_view(&bv);
        assert_eq!(block.round, 7);
        assert_eq!(block.cert_hash, "0xab");
        // The live path stores raw hex (no 0x prefix); backfill
        // matches that to keep the GIN index lookup consistent.
        assert_eq!(block.tx_hashes, vec!["deadbeef", "feedface"]);
    }

    #[tokio::test]
    async fn block_from_view_handles_empty_intents() {
        let store = InMemoryStore::new();
        let bv = BlockView {
            round: 7,
            cert_hash: "0xab".into(),
            intents: vec![],
            tx_hashes: vec![],
        };
        store.ingest_committed_block(block_from_view(&bv)).await;
        assert_eq!(store.latest_round().await, Some(7));
    }
}
