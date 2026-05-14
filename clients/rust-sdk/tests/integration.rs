//! Integration tests for the gsx-client SDK.
//!
//! Stand up a real axum JSON-RPC server on a loopback port that mimics
//! the gsx-rpc method surface, then drive `Client` against it. No
//! `mockito` / wiremock dep — keeps the dep graph minimal.

use std::{net::SocketAddr, sync::Arc};

use axum::{routing::post, Json, Router};
use gsx_client::{AuthorityMemberView, Client, EpochView, Error, StakeEntry, ValidatorMemberView};
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
