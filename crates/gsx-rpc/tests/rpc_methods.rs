//! End-to-end test of the JSON-RPC dispatch surface against a mock
//! `StateView`. Drives the axum `Router` via `tower::ServiceExt::oneshot`
//! to avoid binding a real TCP socket (deterministic; no port conflicts
//! on CI).

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use gsx_rpc::{
    context::{
        AuthorityMemberView, BlockView, EpochView, IntentView, RpcContext, StateView,
        TransactionView, ValidatorMemberView,
    },
    router,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

/// Deterministic in-memory state for the test.
struct MockState {
    epoch: EpochView,
    authorities: Vec<AuthorityMemberView>,
    validators: Vec<ValidatorMemberView>,
    stakes: std::collections::BTreeMap<u32, u128>,
    balances: std::collections::BTreeMap<[u8; 20], u128>,
    blocks_by_round: std::collections::BTreeMap<u64, BlockView>,
    tx_by_hash: std::collections::BTreeMap<[u8; 32], TransactionView>,
}

impl StateView for MockState {
    async fn epoch_snapshot(&self) -> EpochView {
        self.epoch.clone()
    }
    async fn authority_snapshot(&self) -> Vec<AuthorityMemberView> {
        self.authorities.clone()
    }
    async fn validator_snapshot(&self) -> Vec<ValidatorMemberView> {
        self.validators.clone()
    }
    async fn stake_for(&self, authority_id: u32) -> Option<u128> {
        self.stakes.get(&authority_id).copied()
    }
    async fn balance_for(&self, address: [u8; 20]) -> u128 {
        self.balances.get(&address).copied().unwrap_or(0)
    }
    async fn block_at_round(&self, round: u64) -> Option<BlockView> {
        self.blocks_by_round.get(&round).cloned()
    }
    async fn transaction_by_hash(&self, tx_hash: [u8; 32]) -> Option<TransactionView> {
        self.tx_by_hash.get(&tx_hash).cloned()
    }
}

fn fixture() -> Arc<RpcContext<MockState>> {
    let mut stakes = std::collections::BTreeMap::new();
    stakes.insert(0, 30_000u128);
    stakes.insert(1, 30_000u128);
    let mut balances = std::collections::BTreeMap::new();
    balances.insert([0xAA; 20], 1_000u128);
    balances.insert([0xBB; 20], 2_500u128);

    // Block index fixture: round 42 has a single Transfer intent.
    let cert_hex = format!("0x{}", "ab".repeat(32));
    let tx_hex = format!("0x{}", "cd".repeat(32));
    let transfer_view = IntentView::Transfer {
        from: format!("0x{}", "11".repeat(20)),
        to: format!("0x{}", "22".repeat(20)),
        amount: "42".into(),
    };
    let block_42 = BlockView {
        round: 42,
        cert_hash: cert_hex.clone(),
        intents: vec![transfer_view.clone()],
    };
    let mut blocks_by_round = std::collections::BTreeMap::new();
    blocks_by_round.insert(42u64, block_42);
    let tx_view = TransactionView {
        tx_hash: tx_hex.clone(),
        round: 42,
        cert_hash: cert_hex.clone(),
        index: 0,
        intent: transfer_view,
    };
    let mut tx_hash_bytes = [0u8; 32];
    tx_hash_bytes.fill(0xCD);
    let mut tx_by_hash = std::collections::BTreeMap::new();
    tx_by_hash.insert(tx_hash_bytes, tx_view);

    Arc::new(RpcContext::new(Arc::new(MockState {
        epoch: EpochView {
            current: 7,
            last_boundary_round: 7168,
            rounds_per_epoch: 1024,
        },
        authorities: vec![
            AuthorityMemberView {
                id: 0,
                stake_gsx: 150_000,
                public_key_hex: "deadbeef".into(),
            },
            AuthorityMemberView {
                id: 1,
                stake_gsx: 150_000,
                public_key_hex: "cafef00d".into(),
            },
        ],
        validators: vec![
            ValidatorMemberView {
                id: 0,
                stake_gsx: "30000".into(),
            },
            ValidatorMemberView {
                id: 1,
                stake_gsx: "30000".into(),
            },
        ],
        stakes,
        balances,
        blocks_by_round,
        tx_by_hash,
    })))
}

async fn post_rpc(ctx: Arc<RpcContext<MockState>>, body: Value) -> Value {
    let app = router(ctx);
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn get_epoch_returns_snapshot() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "gsx_getEpoch",
        }),
    )
    .await;

    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["result"]["current"], 7);
    assert_eq!(resp["result"]["last_boundary_round"], 7168);
    assert_eq!(resp["result"]["rounds_per_epoch"], 1024);
    assert!(resp["error"].is_null());
}

#[tokio::test]
async fn get_authority_registry_returns_ordered_list() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": "two",
            "method": "gsx_getAuthorityRegistry",
        }),
    )
    .await;

    assert_eq!(resp["id"], "two");
    let arr = resp["result"].as_array().expect("result is array");
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["id"], 0);
    assert_eq!(arr[0]["public_key_hex"], "deadbeef");
    assert_eq!(arr[1]["id"], 1);
    assert_eq!(arr[1]["public_key_hex"], "cafef00d");
}

#[tokio::test]
async fn get_validator_registry_returns_stake_as_string() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "gsx_getValidatorRegistry",
        }),
    )
    .await;

    let arr = resp["result"].as_array().unwrap();
    assert_eq!(arr.len(), 2);
    // JSON-safe u128 encoding: decimal string, not number.
    assert_eq!(arr[0]["stake_gsx"], "30000");
    assert!(arr[0]["stake_gsx"].is_string());
}

#[tokio::test]
async fn get_stake_object_params() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "gsx_getStake",
            "params": { "authority_id": 1 },
        }),
    )
    .await;

    assert_eq!(resp["result"]["id"], 1);
    assert_eq!(resp["result"]["stake_gsx"], "30000");
}

#[tokio::test]
async fn get_stake_positional_params() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "gsx_getStake",
            "params": [0],
        }),
    )
    .await;

    assert_eq!(resp["result"]["id"], 0);
    assert_eq!(resp["result"]["stake_gsx"], "30000");
}

#[tokio::test]
async fn get_stake_not_found() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "gsx_getStake",
            "params": { "authority_id": 999 },
        }),
    )
    .await;

    // Application-level NotFound — code -32000.
    assert_eq!(resp["error"]["code"], -32000);
    assert!(resp["result"].is_null());
}

#[tokio::test]
async fn method_not_found() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "gsx_bogus",
        }),
    )
    .await;

    assert_eq!(resp["error"]["code"], -32601);
    assert!(resp["result"].is_null());
}

#[tokio::test]
async fn get_balance_with_0x_prefix() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": "gsx_getBalance",
            "params": { "address": format!("0x{}", "aa".repeat(20)) },
        }),
    )
    .await;

    assert_eq!(resp["result"]["address"], format!("0x{}", "aa".repeat(20)));
    assert_eq!(resp["result"]["balance"], "1000");
}

#[tokio::test]
async fn get_balance_without_0x_prefix() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 10,
            "method": "gsx_getBalance",
            "params": { "address": "bb".repeat(20) },
        }),
    )
    .await;

    assert_eq!(resp["result"]["balance"], "2500");
}

#[tokio::test]
async fn get_balance_positional_params() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 11,
            "method": "gsx_getBalance",
            "params": [format!("0x{}", "aa".repeat(20))],
        }),
    )
    .await;

    assert_eq!(resp["result"]["balance"], "1000");
}

#[tokio::test]
async fn get_balance_unknown_address_returns_zero() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 12,
            "method": "gsx_getBalance",
            "params": { "address": format!("0x{}", "cd".repeat(20)) },
        }),
    )
    .await;

    // Substrate doesn't distinguish absent from explicit-zero; an unknown
    // address must return balance=0 (not a NotFound error).
    assert_eq!(resp["result"]["balance"], "0");
    assert!(resp["error"].is_null());
}

#[tokio::test]
async fn get_balance_bad_hex_is_invalid_params() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 13,
            "method": "gsx_getBalance",
            "params": { "address": "0xzznotahexstring" },
        }),
    )
    .await;

    // -32602 == InvalidParams
    assert_eq!(resp["error"]["code"], -32602);
}

#[tokio::test]
async fn get_balance_wrong_length_is_invalid_params() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 14,
            "method": "gsx_getBalance",
            "params": { "address": "0xdeadbeef" },  // 4 bytes, not 20
        }),
    )
    .await;

    assert_eq!(resp["error"]["code"], -32602);
}

#[tokio::test]
async fn get_block_object_params() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 20,
            "method": "gsx_getBlock",
            "params": { "round": 42 },
        }),
    )
    .await;

    assert_eq!(resp["result"]["round"], 42);
    assert_eq!(
        resp["result"]["cert_hash"],
        format!("0x{}", "ab".repeat(32))
    );
    let intents = resp["result"]["intents"].as_array().unwrap();
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0]["kind"], "transfer");
    assert_eq!(intents[0]["amount"], "42");
}

#[tokio::test]
async fn get_block_positional_params() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 21,
            "method": "gsx_getBlock",
            "params": [42],
        }),
    )
    .await;

    assert_eq!(resp["result"]["round"], 42);
}

#[tokio::test]
async fn get_block_not_found() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 22,
            "method": "gsx_getBlock",
            "params": { "round": 999 },
        }),
    )
    .await;

    // -32000 application-level NotFound (consistent with gsx_getStake).
    assert_eq!(resp["error"]["code"], -32000);
}

#[tokio::test]
async fn get_transaction_by_hash() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 23,
            "method": "gsx_getTransaction",
            "params": { "tx_hash": format!("0x{}", "cd".repeat(32)) },
        }),
    )
    .await;

    assert_eq!(resp["result"]["tx_hash"], format!("0x{}", "cd".repeat(32)));
    assert_eq!(resp["result"]["round"], 42);
    assert_eq!(resp["result"]["index"], 0);
    assert_eq!(resp["result"]["intent"]["kind"], "transfer");
}

#[tokio::test]
async fn get_transaction_not_found() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 24,
            "method": "gsx_getTransaction",
            "params": { "tx_hash": format!("0x{}", "ff".repeat(32)) },
        }),
    )
    .await;

    assert_eq!(resp["error"]["code"], -32000);
}

#[tokio::test]
async fn get_transaction_bad_hash_is_invalid_params() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 25,
            "method": "gsx_getTransaction",
            "params": { "tx_hash": "0xdeadbeef" },  // 4 bytes, not 32
        }),
    )
    .await;

    assert_eq!(resp["error"]["code"], -32602);
}

#[tokio::test]
async fn invalid_jsonrpc_version() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "1.0",
            "id": 8,
            "method": "gsx_getEpoch",
        }),
    )
    .await;

    assert_eq!(resp["error"]["code"], -32600);
}
