//! JSON-RPC 2.0 query API for the gsx-dag node (Phase 2.1 MVP).
//!
//! The transport is a single POST endpoint at `/` accepting a JSON-RPC
//! request body and returning a JSON-RPC response body. WebSocket /
//! subscription transport is out of scope for this MVP — `subscribeEvents`
//! lands in a follow-on PR with `axum::extract::ws`.
//!
//! Methods (read-only, P1 surface for tooling + indexer bootstrap):
//!
//! - `gsx_getEpoch` — current epoch + last boundary round + rounds_per_epoch
//! - `gsx_getAuthorityRegistry` — ordered list of Authority Ring members
//! - `gsx_getValidatorRegistry` — ordered list of Validator Ring members
//! - `gsx_getStake { authority_id }` — stake for a specific authority id
//!
//! Deferred to follow-on PRs (each touches state not currently indexed
//! for fast lookup, or duplicates an existing write path):
//!
//! - `gsx_getBlock`, `gsx_getTransaction` — need a queryable index over
//!   `state.blocks` (round → block, tx-hash → block).
//! - `gsx_getBalance` — needs a substrate-state read API.
//! - `gsx_submitIntent` — duplicates the existing `ClientMessage::Submit`
//!   write path; merging the two needs careful design.
//! - `gsx_subscribeEvents` (WS) — needs WebSocket transport.

pub mod context;
pub mod error;
pub mod methods;
pub mod router;
pub mod types;

pub use context::{RpcContext, StateView};
pub use error::RpcError;
pub use router::router;
pub use types::{JsonRpcRequest, JsonRpcResponse};

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::task::JoinHandle;
use tracing::info;

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
    let app = router(ctx);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let actual = listener.local_addr()?;
    info!(addr = %actual, "gsx-rpc server bound");
    let handle = tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, app).await {
            tracing::error!(error = %err, "gsx-rpc server exited");
        }
    });
    Ok(handle)
}
