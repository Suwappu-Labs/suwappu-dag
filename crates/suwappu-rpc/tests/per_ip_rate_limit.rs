//! B2.1 integration test: per-IP rate limit emits a JSON-RPC
//! `-32099 RateLimited` envelope and isolates buckets across IPs.
//!
//! Wired through `tower::ServiceExt::oneshot` against the assembled
//! router with `ConnectInfo<SocketAddr>` injected as a request
//! extension — no real TCP socket, so the test is deterministic and
//! independent of CI port allocation.

use std::{net::SocketAddr, sync::Arc};

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use suwappu_rpc::{
    context::{
        AuthorityMemberView, BlockView, EpochView, RpcContext, StateView, SubmitIntentError,
        TransactionView, ValidatorMemberView,
    },
    router_with_limits, RouterLimits,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

/// Minimal `StateView` — every method returns trivial data; we only
/// exercise the rate-limit layer here.
struct MockState;

impl StateView for MockState {
    async fn epoch_snapshot(&self) -> EpochView {
        EpochView {
            current: 0,
            last_boundary_round: 0,
            rounds_per_epoch: 1024,
            latest_committed_round: 0,
        }
    }
    async fn authority_snapshot(&self) -> Vec<AuthorityMemberView> {
        Vec::new()
    }
    async fn validator_snapshot(&self) -> Vec<ValidatorMemberView> {
        Vec::new()
    }
    async fn stake_for(&self, _authority_id: u32) -> Option<u128> {
        None
    }
    async fn balance_for(&self, _address: [u8; 20]) -> u128 {
        0
    }
    async fn block_at_round(&self, _round: u64) -> Option<BlockView> {
        None
    }
    async fn transaction_by_hash(&self, _tx_hash: [u8; 32]) -> Option<TransactionView> {
        None
    }
    async fn submit_intent(
        &self,
        _intent_bincode: Vec<u8>,
        _signature: Vec<u8>,
        _signer_pubkey_hash: [u8; 32],
        _signer_pubkey: Option<Vec<u8>>,
    ) -> Result<[u8; 32], SubmitIntentError> {
        Ok([0u8; 32])
    }
    async fn l1_state_root(&self) -> [u8; 32] {
        [0u8; 32]
    }
    async fn l2_state_root(&self, _l2_chain_id_hash: [u8; 32]) -> [u8; 32] {
        [0u8; 32]
    }
    async fn force_include_registry_bytes(&self) -> Vec<u8> {
        Vec::new()
    }
    fn subscribe_events(&self) -> tokio::sync::broadcast::Receiver<suwappu_rpc::context::EventView> {
        // Unused in this test; return a fresh broadcast pair.
        let (tx, rx) = tokio::sync::broadcast::channel(1);
        // Keep tx alive so the receiver doesn't immediately error.
        std::mem::forget(tx);
        rx
    }
}

fn ctx() -> Arc<RpcContext<MockState>> {
    Arc::new(RpcContext::new(Arc::new(MockState)))
}

async fn call_from(app: axum::Router, peer: SocketAddr, body: Value) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    // Inject the `ConnectInfo<SocketAddr>` extension that the
    // per-IP middleware reads. In production this is populated by
    // `axum::serve(_, app.into_make_service_with_connect_info())`;
    // for the test we set it directly.
    req.extensions_mut().insert(ConnectInfo(peer));
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

fn rpc_get_epoch(id: u64) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "suwappu_getEpoch",
        "params": {},
    })
}

#[tokio::test]
async fn exceeding_capacity_returns_rate_limited_envelope() {
    // Tight bucket — capacity 3, refill 1/s. The 4th call within a
    // second from the same IP must reject.
    let limits = RouterLimits {
        per_ip_capacity: 3,
        per_ip_refill_per_sec: 1,
        ..RouterLimits::default()
    };
    let app = router_with_limits(ctx(), limits);
    let peer: SocketAddr = "10.0.0.1:33333".parse().unwrap();

    for i in 0..3 {
        let (status, body) = call_from(app.clone(), peer, rpc_get_epoch(i)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            body.get("result").is_some(),
            "call {i} should succeed; got {body}"
        );
    }

    let (status, body) = call_from(app, peer, rpc_get_epoch(99)).await;
    assert_eq!(status, StatusCode::OK, "rejection still uses HTTP 200");
    let code = body
        .get("error")
        .and_then(|e| e.get("code"))
        .and_then(|c| c.as_i64())
        .unwrap_or_else(|| panic!("expected error envelope, got {body}"));
    assert_eq!(code, -32099, "must be RpcError::RateLimited code");
    let id = body.get("id").cloned().unwrap_or(Value::Null);
    assert_eq!(id, json!(99), "rejection envelope must echo the request id");
}

#[tokio::test]
async fn buckets_are_independent_across_ips() {
    let limits = RouterLimits {
        per_ip_capacity: 2,
        per_ip_refill_per_sec: 1,
        ..RouterLimits::default()
    };
    let app = router_with_limits(ctx(), limits);
    let ip_a: SocketAddr = "10.0.0.1:11111".parse().unwrap();
    let ip_b: SocketAddr = "10.0.0.2:22222".parse().unwrap();

    // Drain IP A entirely.
    for i in 0..2 {
        let (_, body) = call_from(app.clone(), ip_a, rpc_get_epoch(i)).await;
        assert!(body.get("result").is_some());
    }
    let (_, rejected) = call_from(app.clone(), ip_a, rpc_get_epoch(10)).await;
    assert_eq!(
        rejected.pointer("/error/code").and_then(|c| c.as_i64()),
        Some(-32099)
    );

    // IP B is untouched.
    for i in 0..2 {
        let (_, body) = call_from(app.clone(), ip_b, rpc_get_epoch(i + 100)).await;
        assert!(
            body.get("result").is_some(),
            "IP B should not be affected by IP A's exhaustion; got {body}"
        );
    }
}

#[tokio::test]
async fn missing_connect_info_admits_request() {
    // No ConnectInfo on the request — the layer is defense-in-depth,
    // not a correctness invariant. Admit and let the global
    // concurrency cap protect resources.
    let app = router_with_limits(
        ctx(),
        RouterLimits {
            per_ip_capacity: 1,
            per_ip_refill_per_sec: 0,
            ..RouterLimits::default()
        },
    );
    let req = Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&rpc_get_epoch(0)).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(body.get("result").is_some(), "should admit; got {body}");
}
