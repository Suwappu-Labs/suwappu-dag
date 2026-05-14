//! WebSocket subscriber. Pulls live `EventView` frames from a
//! gsx-rpc node and feeds the local store.
//!
//! Lifecycle:
//!
//! 1. Connect to `ws_url`.
//! 2. For each text frame, parse as `EventView`. If it's a `lagged`
//!    notice, log + continue (MVP doesn't trigger a backfill — that
//!    lands in T7). If it's a committed event, convert and ingest.
//! 3. On socket close or error, sleep `reconnect_secs` and retry.

use std::time::Duration;

use futures_util::StreamExt;
use gsx_rpc::context::EventView;
use serde::Deserialize;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, info, warn};

use crate::store::{block_from_committed_event, Store};

/// Lagged-notice envelope sent by the gsx-rpc WebSocket handler
/// when a subscriber falls behind the broadcast buffer.
#[derive(Debug, Deserialize)]
struct LaggedNotice {
    /// Will always equal `"lagged"` when this notice is sent.
    error: String,
    #[serde(default)]
    skipped: u64,
}

/// Run the subscribe loop forever. Each disconnect/error triggers a
/// reconnect after `reconnect_secs`. Returns only if `store` is
/// dropped (in practice, never — `Arc<Store>` outlives this task).
pub async fn run<S: Store>(ws_url: String, store: S, reconnect_secs: u64) {
    loop {
        match connect_async(&ws_url).await {
            Ok((stream, _resp)) => {
                info!(url = %ws_url, "indexer: connected");
                let mut consumed = 0u64;
                let (_write, mut read) = stream.split();
                while let Some(msg) = read.next().await {
                    match msg {
                        Ok(Message::Text(line)) => {
                            consumed += 1;
                            handle_frame(&line, &store).await;
                        }
                        Ok(Message::Close(_)) => {
                            debug!("indexer: peer closed");
                            break;
                        }
                        Ok(Message::Ping(_) | Message::Pong(_) | Message::Binary(_)) => {
                            // Out of scope for the MVP. Tungstenite
                            // handles ping/pong internally; binary
                            // frames aren't part of the protocol.
                        }
                        Ok(Message::Frame(_)) => {}
                        Err(e) => {
                            warn!(error = %e, "indexer: stream error; reconnecting");
                            break;
                        }
                    }
                }
                info!(consumed, "indexer: socket closed; reconnecting");
            }
            Err(e) => {
                warn!(error = %e, url = %ws_url, "indexer: connect failed");
            }
        }
        tokio::time::sleep(Duration::from_secs(reconnect_secs)).await;
    }
}

async fn handle_frame<S: Store>(line: &str, store: &S) {
    // First try the lagged notice — narrow shape, fastest path.
    if let Ok(notice) = serde_json::from_str::<LaggedNotice>(line) {
        if notice.error == "lagged" {
            warn!(
                skipped = notice.skipped,
                "indexer: server reports lagged events; backfill needed (T7 follow-up)"
            );
            return;
        }
    }
    // Otherwise it's an EventView.
    let ev: EventView = match serde_json::from_str(line) {
        Ok(e) => e,
        Err(e) => {
            warn!(error = %e, "indexer: bad frame, skipping");
            return;
        }
    };
    if let Some(block) = block_from_committed_event(&ev) {
        store.ingest_committed_block(block).await;
    }
    // Non-committed events (proposed, voted, received, …) are
    // recorded by the daemon's NDJSON log; the indexer's commit-only
    // store keeps the schema tight. Full event archival is a T7
    // option if explorers want a full audit log.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{InMemoryStore, IndexedBlock};

    #[tokio::test]
    async fn handle_frame_ingests_committed_event() {
        let store = InMemoryStore::new();
        let frame = r#"{"t_ms":1,"region":"v0","lane":"main","event":"committed","round":42,"cert_hash":"0xab","intent_hashes":["0xcd","0xef"]}"#;
        handle_frame(frame, &store).await;
        let got = store.get_block(42).await.unwrap();
        assert_eq!(got.round, 42);
        assert_eq!(got.cert_hash, "0xab");
        assert_eq!(got.tx_hashes.len(), 2);
    }

    #[tokio::test]
    async fn handle_frame_ignores_lagged_notice() {
        let store = InMemoryStore::new();
        let frame = r#"{"error":"lagged","skipped":3,"skipped_total":3}"#;
        handle_frame(frame, &store).await;
        assert_eq!(store.latest_round().await, None);
    }

    #[tokio::test]
    async fn handle_frame_skips_non_committed() {
        let store = InMemoryStore::new();
        let frame = r#"{"t_ms":1,"region":"v0","lane":"main","event":"proposed","round":1}"#;
        handle_frame(frame, &store).await;
        assert_eq!(store.latest_round().await, None);
    }

    #[tokio::test]
    async fn handle_frame_skips_malformed() {
        let store = InMemoryStore::new();
        handle_frame("not json at all", &store).await;
        assert_eq!(store.latest_round().await, None);
    }

    #[test]
    fn unused_warn_imports_compile_cleanly() {
        // Smoke test that the `Store` trait bound resolves and
        // `InMemoryStore` satisfies it. Compile-only.
        fn _bounds<S: Store>() {}
        _bounds::<InMemoryStore>();
        let _ = IndexedBlock {
            round: 0,
            indexed_at_ms: 0,
            cert_hash: String::new(),
            tx_hashes: vec![],
        };
    }
}
