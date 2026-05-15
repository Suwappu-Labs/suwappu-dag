//! JSON-RPC 2.0 query API for the gsx-dag node.
//!
//! Two transports:
//!
//! - `POST /` — JSON-RPC request/response for all unary methods
//!   (read + write).
//! - `GET /ws` — WebSocket subscription for `gsx_subscribeEvents`
//!   live event stream (T6).
//!
//! Method surface (current):
//!
//! Read:
//! - `gsx_getEpoch`
//! - `gsx_getAuthorityRegistry`
//! - `gsx_getValidatorRegistry`
//! - `gsx_getStake { authority_id }`
//! - `gsx_getBalance { address: hex }`
//! - `gsx_getBlock { round }`
//! - `gsx_getTransaction { tx_hash: hex }`
//!
//! Write:
//! - `gsx_submitIntent { intent: hex, signature: hex, signer_pubkey_hash: hex }`
//!
//! Streaming (WebSocket):
//! - `gsx_subscribeEvents` — live stream of every emitted event
//!   (one JSON `EventView` per text frame; `{"error":"lagged",...}`
//!   on backpressure).

pub mod context;
pub mod error;
pub mod methods;
pub mod per_ip;
pub mod router;
pub mod types;
pub mod ws;

use std::{net::SocketAddr, sync::Arc};

pub use context::{RpcContext, StateView};
pub use error::RpcError;
pub use router::{router, router_with_limits, RouterLimits};
use tokio::task::JoinHandle;
use tracing::info;
pub use types::{JsonRpcRequest, JsonRpcResponse};

/// Spawn the JSON-RPC server bound to `addr`. Returns a join handle on
/// the listener task. The task runs until the process exits or the
/// handle is dropped.
///
/// `ctx` carries the read-only handles into the node state (registries,
/// stake table, epoch). The same `Arc<State>` shared by the rest of
/// `gsx-node` is wrapped in `RpcContext::from_state` in the daemon's
/// bind site.
pub async fn start<S: StateView>(
    addr: SocketAddr,
    ctx: Arc<RpcContext<S>>,
) -> anyhow::Result<JoinHandle<()>> {
    start_with_limits(addr, ctx, RouterLimits::default()).await
}

/// Variant of [`start`] that takes explicit hardening limits. Daemon
/// callers read these from `NodeConfig`.
pub async fn start_with_limits<S: StateView>(
    addr: SocketAddr,
    ctx: Arc<RpcContext<S>>,
    limits: RouterLimits,
) -> anyhow::Result<JoinHandle<()>> {
    let app = router_with_limits(ctx, limits);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let actual = listener.local_addr()?;
    info!(addr = %actual, "gsx-rpc server bound");
    // `into_make_service_with_connect_info::<SocketAddr>()` populates
    // the `ConnectInfo<SocketAddr>` request extension that the
    // per-IP middleware extracts. Without this, the middleware sees
    // no extension and admits every request (defense-in-depth: the
    // global concurrency cap still applies).
    let make_service = app.into_make_service_with_connect_info::<SocketAddr>();
    let handle = tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, make_service).await {
            tracing::error!(error = %err, "gsx-rpc server exited");
        }
    });
    Ok(handle)
}
