//! WebSocket handler for `gsx_subscribeEvents` (T6).
//!
//! Clients connect to `/ws` and receive a stream of NDJSON-shaped
//! event messages (one `EventView` JSON object per WebSocket text
//! message). The stream is **live-only** — there's no replay of past
//! events. Consumers that need backfill should query
//! `gsx_getBlock` / `gsx_getTransaction` for the last known
//! checkpoint, then subscribe.
//!
//! ## Backpressure policy
//!
//! The underlying `broadcast` channel has a fixed ring buffer (1024
//! slots on the daemon side — see `crates/gsx-node/src/events.rs`).
//! A slow consumer eventually trips `RecvError::Lagged` from
//! `Receiver::recv`. We surface that to the WS peer as a single text
//! frame `{"error":"lagged","skipped":N}` and then keep streaming
//! from the new tail. Indexers should treat `lagged` as a signal to
//! reconnect and refill via `gsx_getBlock` from their checkpoint.
//!
//! ## Backpressure on the socket itself
//!
//! `axum::extract::ws::WebSocket::send` is async; if a client is
//! slow to drain, our `send().await` will yield, naturally pacing
//! the broadcast pump. There's no `try_send` here.

use std::sync::Arc;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use tokio::sync::broadcast::error::RecvError;
use tracing::{debug, warn};

use crate::context::{RpcContext, StateView};

/// `GET /ws` — upgrade to WebSocket and stream live events.
pub async fn handle_ws_upgrade<S: StateView>(
    State(ctx): State<Arc<RpcContext<S>>>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let rx = ctx.state.subscribe_events();
    ws.on_upgrade(move |socket| pump_events(socket, rx))
}

async fn pump_events(
    mut socket: WebSocket,
    mut rx: tokio::sync::broadcast::Receiver<crate::context::EventView>,
) {
    let mut skipped_total: u64 = 0;
    loop {
        match rx.recv().await {
            Ok(ev) => {
                let line = match serde_json::to_string(&ev) {
                    Ok(s) => s,
                    Err(e) => {
                        warn!(error = %e, "ws: failed to serialize EventView; dropping");
                        continue;
                    }
                };
                if socket.send(Message::Text(line)).await.is_err() {
                    debug!("ws: peer disconnected");
                    return;
                }
            }
            Err(RecvError::Lagged(skipped)) => {
                skipped_total = skipped_total.saturating_add(skipped);
                let notice = serde_json::json!({
                    "error": "lagged",
                    "skipped": skipped,
                    "skipped_total": skipped_total,
                });
                let _ = socket.send(Message::Text(notice.to_string())).await;
                // Stay connected; broadcast::Receiver auto-resyncs to
                // the next available slot after a Lagged error.
            }
            Err(RecvError::Closed) => {
                debug!("ws: event channel closed (daemon shutdown?)");
                let _ = socket.close().await;
                return;
            }
        }
    }
}
