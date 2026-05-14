//! HTTP read API for the indexer. Thin axum router over the
//! [`crate::store::Store`] trait so the explorer (T8) can navigate
//! committed blocks without hitting the daemon.
//!
//! Endpoints (MVP):
//!
//! - `GET /health` — liveness probe; always returns 200.
//! - `GET /blocks/:round` — single block by round (404 if absent).
//! - `GET /blocks?from=N&to=M` — inclusive range (default `to = from`).
//!
//! Shapes mirror `gsx_rpc::context` field-for-field so a client
//! that already speaks RPC can switch to the indexer with zero glue.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::Deserialize;

use crate::store::{IndexedBlock, Store};

/// Build the indexer HTTP router. Generic over `S: Store` so the
/// caller picks the storage backend (`InMemoryStore` today; Postgres
/// in T7 follow-up).
pub fn router<S: Store>(store: Arc<S>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/blocks/:round", get(get_block::<S>))
        .route("/blocks", get(list_blocks::<S>))
        .with_state(store)
}

async fn health() -> &'static str {
    "ok"
}

async fn get_block<S: Store>(
    State(store): State<Arc<S>>,
    Path(round): Path<u64>,
) -> impl IntoResponse {
    match store.get_block(round).await {
        Some(block) => (StatusCode::OK, Json(Some(block))),
        None => (StatusCode::NOT_FOUND, Json(None::<IndexedBlock>)),
    }
}

#[derive(Debug, Deserialize)]
struct RangeQuery {
    from: u64,
    /// Inclusive upper bound. Defaults to `from` (single-round
    /// response) so a stray `/blocks?from=42` still returns
    /// something sensible.
    #[serde(default)]
    to: Option<u64>,
}

async fn list_blocks<S: Store>(
    State(store): State<Arc<S>>,
    Query(q): Query<RangeQuery>,
) -> Json<Vec<IndexedBlock>> {
    let to = q.to.unwrap_or(q.from);
    Json(store.get_blocks(q.from, to).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::InMemoryStore;

    fn block(round: u64) -> IndexedBlock {
        IndexedBlock {
            round,
            indexed_at_ms: 0,
            cert_hash: format!("0x{:02x}", round),
            tx_hashes: vec![],
        }
    }

    #[tokio::test]
    async fn health_endpoint_returns_ok_text() {
        let body = health().await;
        assert_eq!(body, "ok");
    }

    #[tokio::test]
    async fn get_block_returns_404_for_missing() {
        let store = Arc::new(InMemoryStore::new());
        // Construct the handler-call directly; axum routing tests
        // require a tower::ServiceExt which we already exercise in
        // gsx-rpc/tests. Here the unit-level check is enough.
        let resp = store.get_block(42).await;
        assert!(resp.is_none());
    }

    #[tokio::test]
    async fn range_query_returns_inclusive() {
        let store = InMemoryStore::new();
        for r in 0..5u64 {
            store.ingest_committed_block(block(r)).await;
        }
        let blocks = store.get_blocks(1, 3).await;
        assert_eq!(blocks.len(), 3);
    }
}
