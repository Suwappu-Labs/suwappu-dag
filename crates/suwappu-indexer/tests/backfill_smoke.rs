//! F2 integration smoke: catch-up backfill walks from the store's
//! latest round to the chain head, ingesting every block in between
//! while ignoring `Skip` rounds and existing entries.
//!
//! Uses a hand-rolled axum mock server (no wiremock dep) that
//! answers `suwappu_getEpoch` and `suwappu_getBlock(round)`. The chain shape
//! is configurable per test: a `Vec<Option<BlockView>>` indexed by
//! round, where `None` means "Skip / no block".

use std::{net::SocketAddr, sync::Arc};

use axum::{extract::State, routing::post, Json, Router};
use suwappu_indexer::{
    backfill::catch_up,
    store::{InMemoryStore, IndexedBlock, Store},
};
use suwappu_rpc::context::{BlockView, EpochView};
use serde_json::{json, Value};
use tokio::net::TcpListener;

#[derive(Clone)]
struct MockChain {
    blocks: Arc<Vec<Option<BlockView>>>,
}

impl MockChain {
    fn head(&self) -> u64 {
        self.blocks.len().saturating_sub(1) as u64
    }
}

async fn handle(State(chain): State<MockChain>, Json(body): Json<Value>) -> Json<Value> {
    let id = body.get("id").cloned().unwrap_or(Value::Null);
    let method = body.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = body.get("params").cloned().unwrap_or(Value::Null);
    let result_or_err = match method {
        "suwappu_getEpoch" => {
            let view = EpochView {
                current: 0,
                last_boundary_round: 0,
                rounds_per_epoch: 1024,
                latest_committed_round: chain.head(),
            };
            Ok(serde_json::to_value(view).unwrap())
        }
        "suwappu_getBlock" => {
            let round = params
                .get("round")
                .and_then(|v| v.as_u64())
                .unwrap_or_default();
            match chain.blocks.get(round as usize).and_then(|b| b.clone()) {
                Some(view) => Ok(serde_json::to_value(view).unwrap()),
                None => Err(("not found", -32000)),
            }
        }
        _ => Err(("method not found", -32601)),
    };
    let resp = match result_or_err {
        Ok(value) => json!({"jsonrpc":"2.0","id":id,"result":value}),
        Err((msg, code)) => json!({
            "jsonrpc":"2.0","id":id,
            "error":{"code":code,"message":msg.to_string()}
        }),
    };
    Json(resp)
}

async fn spawn_mock(chain: MockChain) -> SocketAddr {
    let app = Router::new().route("/", post(handle)).with_state(chain);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

fn bv(round: u64, cert_hex: &str, tx_hashes: Vec<&str>) -> BlockView {
    BlockView {
        round,
        cert_hash: format!("0x{}", cert_hex),
        intents: vec![],
        tx_hashes: tx_hashes.into_iter().map(|s| s.to_string()).collect(),
    }
}

#[tokio::test]
async fn catch_up_fills_gap_from_zero() {
    let chain = MockChain {
        blocks: Arc::new(vec![
            Some(bv(0, "00", vec!["0xaa"])),
            Some(bv(1, "01", vec!["0xbb", "0xcc"])),
            Some(bv(2, "02", vec![])),
            Some(bv(3, "03", vec!["0xdd"])),
        ]),
    };
    let addr = spawn_mock(chain).await;
    let store = InMemoryStore::new();
    let ingested = catch_up(&store, &format!("http://{addr}"), 2)
        .await
        .unwrap();
    assert_eq!(ingested, 4);
    assert_eq!(store.latest_round().await, Some(3));
    // Rows resolve back from the store.
    let b1 = store.get_block(1).await.unwrap();
    assert_eq!(b1.cert_hash, "0x01");
    // tx hashes are stored as raw hex (0x prefix stripped).
    assert_eq!(b1.tx_hashes, vec!["bb".to_string(), "cc".to_string()]);
}

#[tokio::test]
async fn catch_up_skips_already_indexed_rounds() {
    let chain = MockChain {
        blocks: Arc::new(vec![
            Some(bv(0, "00", vec![])),
            Some(bv(1, "01", vec![])),
            Some(bv(2, "02", vec![])),
            Some(bv(3, "03", vec![])),
            Some(bv(4, "04", vec![])),
        ]),
    };
    let addr = spawn_mock(chain).await;
    let store = InMemoryStore::new();
    // Pre-seed the store with rounds 0..=2.
    for r in 0..=2u64 {
        store
            .ingest_committed_block(IndexedBlock {
                round: r,
                indexed_at_ms: 1,
                cert_hash: format!("0x{:02x}", r),
                tx_hashes: vec![],
            })
            .await;
    }
    let ingested = catch_up(&store, &format!("http://{addr}"), 8)
        .await
        .unwrap();
    // Only rounds 3 and 4 land via backfill.
    assert_eq!(ingested, 2);
    assert_eq!(store.latest_round().await, Some(4));
}

#[tokio::test]
async fn catch_up_handles_skip_rounds() {
    // Round 2 is Skip — Mysticeti-C legitimate gap.
    let chain = MockChain {
        blocks: Arc::new(vec![
            Some(bv(0, "00", vec![])),
            Some(bv(1, "01", vec![])),
            None,
            Some(bv(3, "03", vec![])),
        ]),
    };
    let addr = spawn_mock(chain).await;
    let store = InMemoryStore::new();
    let ingested = catch_up(&store, &format!("http://{addr}"), 8)
        .await
        .unwrap();
    assert_eq!(ingested, 3, "Skip round should be elided, not error");
    assert_eq!(store.latest_round().await, Some(3));
    // Round 2 absent from the store.
    assert!(store.get_block(2).await.is_none());
}

#[tokio::test]
async fn catch_up_with_store_ahead_of_chain_is_noop() {
    let chain = MockChain {
        blocks: Arc::new(vec![Some(bv(0, "00", vec![]))]),
    };
    let addr = spawn_mock(chain).await;
    let store = InMemoryStore::new();
    // Store is "ahead" — round 5 already indexed, chain head is 0.
    store
        .ingest_committed_block(IndexedBlock {
            round: 5,
            indexed_at_ms: 1,
            cert_hash: "0x05".into(),
            tx_hashes: vec![],
        })
        .await;
    let ingested = catch_up(&store, &format!("http://{addr}"), 8)
        .await
        .unwrap();
    assert_eq!(ingested, 0);
    assert_eq!(store.latest_round().await, Some(5));
}
