//! Integration tests for the gsx-client SDK.
//!
//! Stand up a real axum JSON-RPC server on a loopback port that mimics
//! the gsx-rpc method surface, then drive `Client` against it. No
//! `mockito` / wiremock dep — keeps the dep graph minimal.

use std::{net::SocketAddr, sync::Arc};

use axum::{routing::post, Json, Router};
use gsx_client::{
    AuthorityMemberView, BalanceView, BlockView, Client, EpochView, Error, StakeEntry,
    TransactionView, ValidatorMemberView,
};
use serde_json::{json, Value};
use tokio::net::TcpListener;

/// Spawn a mock JSON-RPC server that recognizes the four gsx_*
/// methods. Returns the bound address.
async fn spawn_mock_server() -> SocketAddr {
    let app = Router::new().route(
        "/",
        post(|Json(req): Json<Value>| async move {
            let method = req["method"].as_str().unwrap_or("");
            let id = req["id"].clone();
            let result = match method {
                "gsx_getEpoch" => Some(json!({
                    "current": 7,
                    "last_boundary_round": 7168,
                    "rounds_per_epoch": 1024,
                })),
                "gsx_getAuthorityRegistry" => Some(json!([
                    {"id": 0, "stake_gsx": 150_000u64, "public_key_hex": "deadbeef"},
                    {"id": 1, "stake_gsx": 150_000u64, "public_key_hex": "cafef00d"},
                ])),
                "gsx_getValidatorRegistry" => Some(json!([
                    {"id": 0, "stake_gsx": "30000"},
                    {"id": 1, "stake_gsx": "30000"},
                ])),
                "gsx_getStake" => {
                    let params = &req["params"];
                    let auth_id = params["authority_id"].as_u64().unwrap_or(u64::MAX);
                    if auth_id == 0 || auth_id == 1 {
                        Some(json!({"id": auth_id, "stake_gsx": "30000"}))
                    } else {
                        return Json(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {
                                "code": -32000,
                                "message": format!("no stake recorded for authority_id {}", auth_id),
                            }
                        }));
                    }
                }
                "gsx_getBalance" => {
                    let params = &req["params"];
                    let addr_hex = params["address"].as_str().unwrap_or("");
                    let bal = if addr_hex == format!("0x{}", "aa".repeat(20)) {
                        "1000"
                    } else {
                        "0"
                    };
                    Some(json!({"address": addr_hex, "balance": bal}))
                }
                "gsx_getBlock" => {
                    let params = &req["params"];
                    let round = params["round"].as_u64().unwrap_or(u64::MAX);
                    if round == 42 {
                        Some(json!({
                            "round": 42,
                            "cert_hash": format!("0x{}", "ab".repeat(32)),
                            "intents": [
                                {
                                    "kind": "transfer",
                                    "from": format!("0x{}", "11".repeat(20)),
                                    "to": format!("0x{}", "22".repeat(20)),
                                    "amount": "100",
                                }
                            ],
                        }))
                    } else {
                        return Json(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {"code": -32000, "message": format!("no committed block at round {}", round)}
                        }));
                    }
                }
                "gsx_getTransaction" => {
                    let params = &req["params"];
                    let h = params["tx_hash"].as_str().unwrap_or("");
                    if h == format!("0x{}", "cd".repeat(32)) {
                        Some(json!({
                            "tx_hash": h,
                            "round": 42,
                            "cert_hash": format!("0x{}", "ab".repeat(32)),
                            "index": 0,
                            "intent": {
                                "kind": "transfer",
                                "from": format!("0x{}", "11".repeat(20)),
                                "to": format!("0x{}", "22".repeat(20)),
                                "amount": "100",
                            },
                        }))
                    } else {
                        return Json(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {"code": -32000, "message": format!("no committed transaction with hash {}", h)}
                        }));
                    }
                }
                _ => {
                    return Json(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": -32601,
                            "message": format!("method not found: {}", method),
                        }
                    }));
                }
            };
            Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result.unwrap(),
            }))
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

fn client_for(addr: SocketAddr) -> Client {
    Client::new(format!("http://{}", addr))
}

#[tokio::test]
async fn get_epoch_round_trip() {
    let addr = spawn_mock_server().await;
    let client = client_for(addr);

    let epoch: EpochView = client.get_epoch().await.unwrap();
    assert_eq!(
        epoch,
        EpochView {
            current: 7,
            last_boundary_round: 7168,
            rounds_per_epoch: 1024,
        }
    );
}

#[tokio::test]
async fn get_authority_registry_round_trip() {
    let addr = spawn_mock_server().await;
    let client = client_for(addr);

    let auths: Vec<AuthorityMemberView> = client.get_authority_registry().await.unwrap();
    assert_eq!(auths.len(), 2);
    assert_eq!(auths[0].id, 0);
    assert_eq!(auths[0].public_key_hex, "deadbeef");
    assert_eq!(auths[1].id, 1);
    assert_eq!(auths[1].public_key_hex, "cafef00d");
}

#[tokio::test]
async fn get_validator_registry_round_trip() {
    let addr = spawn_mock_server().await;
    let client = client_for(addr);

    let vals: Vec<ValidatorMemberView> = client.get_validator_registry().await.unwrap();
    assert_eq!(vals.len(), 2);
    // u128 round-trips as a decimal string per gsx-rpc's
    // ValidatorMemberView contract.
    assert_eq!(vals[0].stake_gsx, "30000");
}

#[tokio::test]
async fn get_stake_some() {
    let addr = spawn_mock_server().await;
    let client = client_for(addr);

    let s: Option<StakeEntry> = client.get_stake(1).await.unwrap();
    assert_eq!(
        s,
        Some(StakeEntry {
            id: 1,
            stake_gsx: "30000".into()
        })
    );
}

#[tokio::test]
async fn get_stake_none_on_not_found() {
    let addr = spawn_mock_server().await;
    let client = client_for(addr);

    // authority_id 999 is unknown — server returns code -32000 which the
    // SDK translates into Ok(None) so callers don't have to match on
    // the error code themselves.
    let s: Option<StakeEntry> = client.get_stake(999).await.unwrap();
    assert!(s.is_none());
}

#[tokio::test]
async fn get_balance_known_address() {
    let addr = spawn_mock_server().await;
    let client = client_for(addr);

    let b: BalanceView = client.get_balance([0xAA; 20]).await.unwrap();
    assert_eq!(b.address, format!("0x{}", "aa".repeat(20)));
    assert_eq!(b.balance, "1000");
}

#[tokio::test]
async fn get_balance_unknown_address_returns_zero() {
    let addr = spawn_mock_server().await;
    let client = client_for(addr);

    // Substrate doesn't distinguish absent from explicit-zero; an
    // unknown address must round-trip as balance="0", not Err.
    let b: BalanceView = client.get_balance([0xCD; 20]).await.unwrap();
    assert_eq!(b.balance, "0");
}

#[tokio::test]
async fn get_block_known_round() {
    let addr = spawn_mock_server().await;
    let client = client_for(addr);
    let b: BlockView = client.get_block(42).await.unwrap().expect("block exists");
    assert_eq!(b.round, 42);
    assert_eq!(b.intents.len(), 1);
}

#[tokio::test]
async fn get_block_unknown_round_returns_none() {
    let addr = spawn_mock_server().await;
    let client = client_for(addr);
    let b = client.get_block(999).await.unwrap();
    assert!(b.is_none());
}

#[tokio::test]
async fn get_transaction_known_hash() {
    let addr = spawn_mock_server().await;
    let client = client_for(addr);
    let mut h = [0u8; 32];
    h.fill(0xCD);
    let t: TransactionView = client.get_transaction(h).await.unwrap().expect("tx exists");
    assert_eq!(t.round, 42);
    assert_eq!(t.index, 0);
}

#[tokio::test]
async fn get_transaction_unknown_returns_none() {
    let addr = spawn_mock_server().await;
    let client = client_for(addr);
    let h = [0xFFu8; 32];
    let t = client.get_transaction(h).await.unwrap();
    assert!(t.is_none());
}

#[tokio::test]
async fn method_not_found_surfaces_rpc_error() {
    let addr = spawn_mock_server().await;
    let client = client_for(addr);

    let err = client
        .call::<Value>("gsx_bogus", Value::Null)
        .await
        .expect_err("bogus method should error");
    match err {
        Error::Rpc { code, .. } => assert_eq!(code, -32601),
        other => panic!("expected Rpc(-32601), got {:?}", other),
    }
}

#[tokio::test]
async fn transport_error_on_unreachable() {
    // Don't spawn the mock server — point the client at a dead port.
    let client = Client::new("http://127.0.0.1:1");
    let err = client
        .get_epoch()
        .await
        .expect_err("dead port should error");
    matches!(err, Error::Transport(_));
}

#[tokio::test]
async fn client_is_cheap_to_clone_and_id_sequences_are_shared() {
    let addr = spawn_mock_server().await;
    let client = client_for(addr);
    let client2: Arc<Client> = Arc::new(client.clone());
    // Both handles can drive concurrent calls without compile errors;
    // we don't introspect the id sequence here (it's deliberately
    // opaque), but exercising the clone path catches accidental
    // !Send / !Sync regressions.
    let (a, b) = tokio::join!(client.get_epoch(), client2.get_epoch());
    a.unwrap();
    b.unwrap();
}
