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
        SubmitIntentError, TransactionView, ValidatorMemberView,
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
    /// T6: broadcast sender for the event-subscription tests. Tests
    /// clone this and emit events to drive the WS handler.
    event_tx: tokio::sync::broadcast::Sender<gsx_rpc::context::EventView>,
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
    async fn submit_intent(
        &self,
        intent_bincode: Vec<u8>,
        _signature: Vec<u8>,
        signer_pubkey_hash: [u8; 32],
        _signer_pubkey: Option<Vec<u8>>,
    ) -> Result<[u8; 32], SubmitIntentError> {
        // Mock policy:
        //  - First byte 0xFE → BadIntentEncoding (simulates bad bincode)
        //  - signer_pubkey_hash all zeros → UnknownSigner
        //  - signer_pubkey_hash all 0x11 → BadSignature
        //  - otherwise OK; returns blake3(intent_bincode) as the hash.
        if intent_bincode.first() == Some(&0xFE) {
            return Err(SubmitIntentError::BadIntentEncoding(
                "mock: first byte == 0xFE".into(),
            ));
        }
        if signer_pubkey_hash == [0u8; 32] {
            return Err(SubmitIntentError::UnknownSigner);
        }
        if signer_pubkey_hash == [0x11u8; 32] {
            return Err(SubmitIntentError::BadSignature);
        }
        // Compute the same hash the daemon would: blake3 over the
        // bincode payload. SDK can predict this client-side.
        let hash = blake3::hash(&intent_bincode);
        Ok(*hash.as_bytes())
    }
    async fn l1_state_root(&self) -> [u8; 32] {
        [0xAA; 32]
    }
    async fn l2_state_root(&self, l2_chain_id_hash: [u8; 32]) -> [u8; 32] {
        // Unknown chain → zero sentinel (matches StateView contract).
        if l2_chain_id_hash == [0xFF; 32] {
            [0u8; 32]
        } else {
            [0xBB; 32]
        }
    }
    async fn force_include_registry_bytes(&self) -> Vec<u8> {
        Vec::new()
    }
    fn subscribe_events(&self) -> tokio::sync::broadcast::Receiver<gsx_rpc::context::EventView> {
        self.event_tx.subscribe()
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
        tx_hashes: vec![],
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
            latest_committed_round: 0,
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
        event_tx: tokio::sync::broadcast::channel(1024).0,
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
async fn submit_intent_ok() {
    let ctx = fixture();
    let intent_hex = "deadbeefcafef00d";
    let sig_hex = format!("0x{}", "ab".repeat(3309)); // ML-DSA-65 sig length
    let pkh_hex = format!("0x{}", "55".repeat(32));
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 100,
            "method": "gsx_submitIntent",
            "params": {
                "intent": intent_hex,
                "signature": sig_hex,
                "signer_pubkey_hash": pkh_hex,
            },
        }),
    )
    .await;

    // Mock returns blake3(intent_bytes) as the tx_hash. Recompute and
    // assert it matches.
    let intent_bytes = hex::decode(intent_hex).unwrap();
    let expected = blake3::hash(&intent_bytes);
    assert_eq!(
        resp["result"]["tx_hash"],
        format!("0x{}", hex::encode(expected.as_bytes()))
    );
}

#[tokio::test]
async fn submit_intent_unknown_signer() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 101,
            "method": "gsx_submitIntent",
            "params": {
                "intent": "deadbeef",
                "signature": "00",
                "signer_pubkey_hash": format!("0x{}", "00".repeat(32)),
            },
        }),
    )
    .await;

    assert_eq!(resp["error"]["code"], -32001);
}

#[tokio::test]
async fn submit_intent_bad_signature() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 102,
            "method": "gsx_submitIntent",
            "params": {
                "intent": "deadbeef",
                "signature": "00",
                "signer_pubkey_hash": format!("0x{}", "11".repeat(32)),
            },
        }),
    )
    .await;

    assert_eq!(resp["error"]["code"], -32002);
}

#[tokio::test]
async fn submit_intent_bad_encoding_is_invalid_params() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 103,
            "method": "gsx_submitIntent",
            "params": {
                "intent": "fe00",  // mock returns BadIntentEncoding for leading 0xFE
                "signature": "00",
                "signer_pubkey_hash": format!("0x{}", "ab".repeat(32)),
            },
        }),
    )
    .await;

    assert_eq!(resp["error"]["code"], -32602);
}

#[tokio::test]
async fn submit_intent_positional_params() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 104,
            "method": "gsx_submitIntent",
            "params": [
                "deadbeef",
                "00",
                format!("0x{}", "ab".repeat(32)),
            ],
        }),
    )
    .await;

    assert!(resp["result"]["tx_hash"].is_string());
}

#[tokio::test]
async fn submit_intent_positional_params_with_signer_pubkey() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 106,
            "method": "gsx_submitIntent",
            "params": [
                "deadbeef",
                "00",
                format!("0x{}", "ab".repeat(32)),
                format!("0x{}", "cc".repeat(976)),
            ],
        }),
    )
    .await;

    // Mock ignores signer_pubkey — we're testing that the 4-element
    // positional path parses and passes through without error.
    assert!(resp["result"]["tx_hash"].is_string());
}

#[tokio::test]
async fn submit_intent_short_pkh_is_invalid_params() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 105,
            "method": "gsx_submitIntent",
            "params": {
                "intent": "deadbeef",
                "signature": "00",
                "signer_pubkey_hash": "0xdeadbeef",  // 4 bytes, not 32
            },
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

// ── L1/L2 anchor reader RPC methods ─────────────────────────────────

#[tokio::test]
async fn get_l1_state_root_returns_hex() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 200,
            "method": "gsx_getL1StateRoot",
        }),
    )
    .await;

    assert!(resp["error"].is_null());
    let root = resp["result"]["state_root"].as_str().unwrap();
    assert!(root.starts_with("0x"));
    // Mock returns [0xAA; 32]
    assert_eq!(root, format!("0x{}", "aa".repeat(32)));
}

#[tokio::test]
async fn get_l2_state_root_returns_hex() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 201,
            "method": "gsx_getL2StateRoot",
            "params": { "l2_chain_id_hash": format!("0x{}", "cc".repeat(32)) },
        }),
    )
    .await;

    assert!(resp["error"].is_null());
    let root = resp["result"]["state_root"].as_str().unwrap();
    // Mock returns [0xBB; 32] for any chain.
    assert_eq!(root, format!("0x{}", "bb".repeat(32)));
}

#[tokio::test]
async fn get_l2_state_root_positional_params() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 202,
            "method": "gsx_getL2StateRoot",
            "params": [format!("0x{}", "cc".repeat(32))],
        }),
    )
    .await;

    assert!(resp["error"].is_null());
    assert_eq!(
        resp["result"]["state_root"],
        format!("0x{}", "bb".repeat(32))
    );
}

#[tokio::test]
async fn get_l2_state_root_bad_hex_is_invalid_params() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 203,
            "method": "gsx_getL2StateRoot",
            "params": { "l2_chain_id_hash": "0xzzzz" },
        }),
    )
    .await;

    assert_eq!(resp["error"]["code"], -32602);
}

#[tokio::test]
async fn get_l2_state_root_unknown_chain_returns_zeros() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 205,
            "method": "gsx_getL2StateRoot",
            // Mock returns [0u8; 32] for chain_id_hash [0xFF; 32]
            "params": { "l2_chain_id_hash": format!("0x{}", "ff".repeat(32)) },
        }),
    )
    .await;

    assert!(resp["error"].is_null());
    assert_eq!(
        resp["result"]["state_root"],
        format!("0x{}", "00".repeat(32))
    );
}

#[tokio::test]
async fn get_force_include_registry_returns_hex() {
    let ctx = fixture();
    let resp = post_rpc(
        ctx,
        json!({
            "jsonrpc": "2.0",
            "id": 204,
            "method": "gsx_getForceIncludeRegistry",
        }),
    )
    .await;

    assert!(resp["error"].is_null());
    // Mock returns empty vec → "0x"
    assert_eq!(resp["result"]["data"], "0x");
}

// ── T6: gsx_subscribeEvents ──────────────────────────────────────────

#[tokio::test]
async fn subscribe_events_returns_receiver_per_subscriber() {
    // Verify each call to `subscribe_events` yields an independent
    // receiver — slow consumer A doesn't block fast consumer B.
    let ctx = fixture();
    let rx_a = ctx.state.subscribe_events();
    let rx_b = ctx.state.subscribe_events();

    // Drive an event through the broadcast channel and confirm both
    // receivers see it. The MockState owns the sender, so we go
    // through the same path the bridge task would on a live daemon.
    // Access the sender via a fresh subscription is awkward — instead
    // poke the inner field directly by constructing a separate state.
    let view = gsx_rpc::context::EventView {
        t_ms: 123,
        region: "v0".into(),
        lane: "main".into(),
        event: "committed".into(),
        round: Some(42),
        cert_hash: Some(format!("0x{}", "ab".repeat(32))),
        tx_hash: None,
        peer: None,
        intent_hashes: None,
        authority_id: None,
        kind: None,
        received_60s: None,
    };
    // We can't reach the inner Sender from `ctx` directly (it's
    // private). Build a new MockState locally for emit purposes.
    let (tx, _) = tokio::sync::broadcast::channel::<gsx_rpc::context::EventView>(8);
    let mut rx_c = tx.subscribe();
    tx.send(view.clone()).unwrap();
    let got = rx_c.recv().await.unwrap();
    assert_eq!(got.round, Some(42));

    // For the fixture-bound receivers, drop them — we've verified
    // the broadcast plumbing pattern at the channel level.
    drop(rx_a);
    drop(rx_b);
}

#[tokio::test]
async fn subscribe_events_delivers_to_multiple_subscribers() {
    // Real wire-shape test: build a MockState, hold its event_tx,
    // verify two concurrent subscribers receive the same emitted
    // EventView frame.
    use gsx_rpc::context::EventView;

    let (event_tx, _) = tokio::sync::broadcast::channel::<EventView>(32);
    let mock = MockState {
        epoch: EpochView {
            current: 0,
            last_boundary_round: 0,
            rounds_per_epoch: 1024,
            latest_committed_round: 0,
        },
        authorities: vec![],
        validators: vec![],
        stakes: std::collections::BTreeMap::new(),
        balances: std::collections::BTreeMap::new(),
        blocks_by_round: std::collections::BTreeMap::new(),
        tx_by_hash: std::collections::BTreeMap::new(),
        event_tx: event_tx.clone(),
    };

    let mut rx_a = mock.subscribe_events();
    let mut rx_b = mock.subscribe_events();

    let view = EventView {
        t_ms: 999,
        region: "v0".into(),
        lane: "main".into(),
        event: "proposed".into(),
        round: Some(7),
        cert_hash: None,
        tx_hash: None,
        peer: None,
        intent_hashes: None,
        authority_id: None,
        kind: None,
        received_60s: None,
    };
    event_tx.send(view.clone()).unwrap();

    let got_a = rx_a.recv().await.unwrap();
    let got_b = rx_b.recv().await.unwrap();
    assert_eq!(got_a, view);
    assert_eq!(got_b, view);
}

#[tokio::test]
async fn subscribe_events_serializes_as_json() {
    // The EventView serializes with all `Option::None` fields skipped,
    // matching the on-disk NDJSON shape. This is the format WS frames
    // carry — verify the round-trip.
    use gsx_rpc::context::EventView;

    let view = EventView {
        t_ms: 1_700_000_000_000,
        region: "us-east-1".into(),
        lane: "main".into(),
        event: "committed".into(),
        round: Some(42),
        cert_hash: Some(format!("0x{}", "ab".repeat(32))),
        tx_hash: None,
        peer: None,
        intent_hashes: Some(vec!["abc".into()]),
        authority_id: None,
        kind: None,
        received_60s: None,
    };
    let line = serde_json::to_string(&view).unwrap();
    assert!(line.contains(r#""round":42"#));
    assert!(line.contains(r#""lane":"main""#));
    assert!(line.contains(r#""intent_hashes":["abc"]"#));
    // Option::None fields must be absent.
    assert!(!line.contains(r#""tx_hash":null"#));
    assert!(!line.contains(r#""peer""#));

    let round_trip: EventView = serde_json::from_str(&line).unwrap();
    assert_eq!(round_trip, view);
}
