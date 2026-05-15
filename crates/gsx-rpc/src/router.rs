//! axum router for JSON-RPC 2.0 over HTTP POST plus T6's WebSocket
//! event subscription endpoint.
//!
//! - `POST /` — JSON-RPC request/response (all unary methods).
//! - `GET /ws` — WebSocket subscription stream (`gsx_subscribeEvents`).
//!
//! Non-POST on `/` → 405. Malformed JSON → 200 with a JSON-RPC
//! InvalidRequest error in the body (per spec — HTTP status stays
//! 200, transport errors are encoded in the response envelope).

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    middleware as axum_middleware,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde_json::Value;
use tracing::warn;

use crate::{
    context::{RpcContext, StateView},
    error::RpcError,
    methods,
    per_ip::{self, PerIpRateLimiter},
    types::{JsonRpcRequest, JsonRpcResponse},
    ws,
};

/// B2 hardening: default cap on the number of in-flight HTTP requests
/// being served concurrently. A patient attacker could otherwise open
/// many connections each holding a slow request open; combined with
/// the body-size cap this bounds the worst-case memory + CPU cost of
/// the ingress. Tunable via [`RouterLimits`].
pub const DEFAULT_MAX_CONCURRENT_REQUESTS: usize = 64;

/// B2 hardening: default cap on a single JSON-RPC request body. The
/// JSON-RPC envelope is at most a few KB for any read method; a
/// `submitIntent` carries a bincoded intent (~1 KB) + an ML-DSA
/// signature (3,309 B) + a 32-byte hash. 1 MiB is a comfortable
/// upper bound that still rejects payload-amplification probing.
pub const DEFAULT_MAX_REQUEST_BODY_BYTES: usize = 1024 * 1024;

/// B2.1 hardening: default per-IP burst allowance (number of requests
/// that can land back-to-back from one source before refill kicks in).
/// 60 covers a typical wallet's startup-time burst of state queries.
pub const DEFAULT_PER_IP_CAPACITY: u64 = 60;

/// B2.1 hardening: default per-IP steady-state refill rate (requests
/// per second). At 10/s the bucket fills in 6 s after exhaustion; a
/// scripted query loop that polls every 100 ms can sustain forever,
/// while abusive flood attempts are throttled below validator-relevant
/// CPU budgets.
pub const DEFAULT_PER_IP_REFILL_PER_SEC: u64 = 10;

/// Hardening limits applied to the gsx-rpc HTTP router. The defaults
/// match `DEFAULT_MAX_CONCURRENT_REQUESTS` and `DEFAULT_MAX_REQUEST_BODY_BYTES`;
/// callers can override at startup if the deployment topology
/// requires it (e.g., a public-facing validator behind a CDN may
/// permit higher concurrency since per-IP smoothing happens upstream).
#[derive(Clone, Copy, Debug)]
pub struct RouterLimits {
    /// Cap on simultaneous in-flight requests across all sources.
    pub max_concurrent_requests: usize,
    /// Cap on a single HTTP request body, applied at the tower-http
    /// layer (before axum's `Json` extractor allocates).
    pub max_request_body_bytes: usize,
    /// B2.1: per-IP token-bucket capacity (burst allowance).
    pub per_ip_capacity: u64,
    /// B2.1: per-IP token-bucket refill rate (steady-state ceiling
    /// in requests/sec).
    pub per_ip_refill_per_sec: u64,
}

impl Default for RouterLimits {
    fn default() -> Self {
        Self {
            max_concurrent_requests: DEFAULT_MAX_CONCURRENT_REQUESTS,
            max_request_body_bytes: DEFAULT_MAX_REQUEST_BODY_BYTES,
            per_ip_capacity: DEFAULT_PER_IP_CAPACITY,
            per_ip_refill_per_sec: DEFAULT_PER_IP_REFILL_PER_SEC,
        }
    }
}

/// Build the axum router with hardening middleware applied. Caller is
/// responsible for `axum::serve`-ing it. Uses [`RouterLimits::default`].
pub fn router<S: StateView>(ctx: Arc<RpcContext<S>>) -> Router {
    router_with_limits(ctx, RouterLimits::default())
}

/// Build the axum router with explicit hardening limits.
///
/// Middleware order (outermost → innermost):
///   1. `RequestBodyLimitLayer` (tower-http) — rejects an HTTP request
///      whose declared `Content-Length` or streamed body exceeds the
///      cap before axum's `Json` extractor allocates.
///   2. `ConcurrencyLimitLayer` (tower) — caps simultaneously-served
///      requests across all sources. The N+1th request is held in the
///      tower service queue; under sustained overload tokio's
///      backpressure surfaces to the TCP layer.
///   3. **B2.1 per-IP rate limit** — buckets by the TCP source IP
///      extracted from `ConnectInfo<SocketAddr>`. Reject path returns
///      HTTP 200 with a JSON-RPC body carrying error code `-32099`
///      ([`RpcError::RateLimited`]) so SDK consumers see a uniform
///      JSON-RPC envelope regardless of which layer rejected. Skipped
///      when no `ConnectInfo` is present (tower-only unit tests).
///   4. The route table itself (`POST /` for JSON-RPC, `GET /ws` for
///      event subscriptions).
///
/// **Note:** the WebSocket path inherits the same middleware stack
/// since it shares the router. The body-size cap doesn't affect the
/// stream after upgrade (it applies only to the upgrade-request body);
/// the concurrency cap counts an active WS subscription against the
/// cap until disconnect, so operators with many subscribers should
/// raise `max_concurrent_requests` accordingly. The per-IP bucket
/// charges one token per upgrade request, not per delivered frame.
pub fn router_with_limits<S: StateView>(ctx: Arc<RpcContext<S>>, limits: RouterLimits) -> Router {
    let per_ip = PerIpRateLimiter::new(limits.per_ip_capacity, limits.per_ip_refill_per_sec);
    Router::new()
        .route("/", post(handle_rpc::<S>))
        .route("/ws", get(ws::handle_ws_upgrade::<S>))
        .layer(axum_middleware::from_fn_with_state(
            per_ip,
            per_ip::middleware,
        ))
        .layer(tower::limit::ConcurrencyLimitLayer::new(
            limits.max_concurrent_requests,
        ))
        .layer(tower_http::limit::RequestBodyLimitLayer::new(
            limits.max_request_body_bytes,
        ))
        .with_state(ctx)
}

async fn handle_rpc<S: StateView>(
    State(ctx): State<Arc<RpcContext<S>>>,
    body: Result<Json<JsonRpcRequest>, axum::extract::rejection::JsonRejection>,
) -> impl IntoResponse {
    let req = match body {
        Ok(Json(r)) => r,
        Err(err) => {
            warn!(error = %err, "rpc: malformed request body");
            // Per JSON-RPC 2.0, the id is "Null" when not parseable.
            let resp = JsonRpcResponse::err(Value::Null, RpcError::InvalidRequest(err.body_text()));
            return (StatusCode::OK, Json(resp));
        }
    };

    if req.jsonrpc != "2.0" {
        let resp = JsonRpcResponse::err(
            req.id.clone(),
            RpcError::InvalidRequest(format!(
                "jsonrpc field must be \"2.0\", got {:?}",
                req.jsonrpc
            )),
        );
        return (StatusCode::OK, Json(resp));
    }

    let result = dispatch(&*ctx.state, &req.method, &req.params).await;
    let resp = match result {
        Ok(value) => JsonRpcResponse::ok(req.id, value),
        Err(err) => JsonRpcResponse::err(req.id, err),
    };
    (StatusCode::OK, Json(resp))
}

/// Method-name → handler. New methods register here.
async fn dispatch<S: StateView>(
    state: &S,
    method: &str,
    params: &Value,
) -> Result<Value, RpcError> {
    match method {
        "gsx_getEpoch" => methods::get_epoch(state, params).await,
        "gsx_getAuthorityRegistry" => methods::get_authority_registry(state, params).await,
        "gsx_getValidatorRegistry" => methods::get_validator_registry(state, params).await,
        "gsx_getStake" => methods::get_stake(state, params).await,
        "gsx_getBalance" => methods::get_balance(state, params).await,
        "gsx_getBlock" => methods::get_block(state, params).await,
        "gsx_getTransaction" => methods::get_transaction(state, params).await,
        "gsx_submitIntent" => methods::submit_intent(state, params).await,
        unknown => Err(RpcError::MethodNotFound(unknown.into())),
    }
}
