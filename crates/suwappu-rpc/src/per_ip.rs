//! B2.1: per-IP leaky-bucket rate limiter for JSON-RPC ingress.
//!
//! Wraps the route table as an `axum::middleware::from_fn_with_state`
//! handler. Reuses `suwappu_mempool::bucket::LeakyBucket` verbatim — same
//! algorithm, same wall-clock-ms timing, same burst-vs-steady-state
//! tradeoff as the mempool's per-peer limiter. Per-IP rather than
//! per-peer because at the RPC ingress we don't have a stable peer
//! identifier: the only thing tower can see before the JSON-RPC
//! `signer_pubkey_hash` reaches the handler is the TCP source IP.
//!
//! The bucket map is keyed by `IpAddr` (not `SocketAddr`) so a single
//! NATed host sharing many ephemeral source ports counts against one
//! bucket, which matches the operator's intent (rate-limit the user,
//! not the kernel socket).
//!
//! Rejected requests return an HTTP 200 carrying a JSON-RPC envelope
//! with `error.code = -32099` (`RpcError::RateLimited`), per the
//! JSON-RPC 2.0 spec (transport stays 200, application errors live in
//! the envelope). Best-effort id extraction from the request body: a
//! malformed body yields `id = null` per spec.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    body::to_bytes,
    extract::{ConnectInfo, Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use suwappu_mempool::LeakyBucket;
use serde_json::Value;

use crate::{error::RpcError, types::JsonRpcResponse};

/// Drop bucket entries that have been idle longer than this. Each
/// bucket is ~64 B, so even a million unique peers is small RAM —
/// but a long-running validator behind a forwarding proxy could
/// accumulate millions of one-shot IPs over weeks. The GC sweep
/// keeps that bounded without affecting steady-state throughput.
const BUCKET_IDLE_TTL_MS: u64 = 300_000;
/// How often the background sweep runs.
const BUCKET_GC_INTERVAL: Duration = Duration::from_secs(60);

/// Shared state for the per-IP rate limiter.
///
/// Construct once at startup with [`PerIpRateLimiter::new`]; clone
/// the `Arc` into the axum router state. The background GC sweep
/// is spawned by `new` and runs for the limiter's lifetime; dropping
/// every `Arc` cancels it.
#[derive(Debug)]
pub struct PerIpRateLimiter {
    capacity: u64,
    refill_per_sec: u64,
    buckets: Mutex<HashMap<IpAddr, LeakyBucket>>,
    /// CIDRs of trusted proxies (ALB, nginx, etc.) whose
    /// `X-Forwarded-For` header is trusted. When the TCP source IP
    /// is NOT in this set, `X-Forwarded-For` is ignored and the
    /// raw TCP source IP is used — preventing direct clients from
    /// spoofing their IP to bypass rate limiting. Empty means "never
    /// trust X-Forwarded-For" (safe default for bare-metal deploys).
    trusted_proxy_ips: Vec<IpAddr>,
}

impl PerIpRateLimiter {
    /// Build a limiter with the given bucket parameters and spawn the
    /// background GC sweep. `capacity` is the burst allowance per IP;
    /// `refill_per_sec` is the steady-state ceiling.
    pub fn new(capacity: u64, refill_per_sec: u64) -> Arc<Self> {
        Self::with_trusted_proxies(capacity, refill_per_sec, vec![])
    }

    /// Build a limiter that trusts `X-Forwarded-For` only when the
    /// TCP source IP is in `trusted_proxies`. For AWS ALB
    /// deployments, pass the ALB's internal IPs (or VPC CIDR
    /// range). For bare-metal, pass `vec![]` (default via `new`).
    ///
    /// **Note:** not yet wired through `NodeConfig` — callers today
    /// always reach this via `new()` which passes `vec![]`. A future
    /// `rpc_trusted_proxy_ips` config field will complete the
    /// wire-through. Until then, nodes behind an ALB rate-limit by
    /// the ALB's IP (safe default; all clients share one bucket).
    pub fn with_trusted_proxies(
        capacity: u64,
        refill_per_sec: u64,
        trusted_proxies: Vec<IpAddr>,
    ) -> Arc<Self> {
        let inner = Arc::new(Self {
            capacity,
            refill_per_sec,
            buckets: Mutex::new(HashMap::new()),
            trusted_proxy_ips: trusted_proxies,
        });
        let gc_handle = Arc::clone(&inner);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(BUCKET_GC_INTERVAL);
            // First tick fires immediately; skip it so we don't sweep an
            // empty map a tick into startup.
            tick.tick().await;
            loop {
                tick.tick().await;
                gc_handle.sweep_idle();
            }
        });
        inner
    }

    /// Try to charge one token for `ip`. Returns `Ok(())` on admit or
    /// `Err(retry_after_ms)` on reject.
    pub fn try_acquire(&self, ip: IpAddr, now_ms: u64) -> Result<(), u64> {
        let mut buckets = self.buckets.lock().expect("per-ip bucket map poisoned");
        let bucket = buckets
            .entry(ip)
            .or_insert_with(|| LeakyBucket::new(self.capacity, self.refill_per_sec, now_ms));
        bucket.take_one(now_ms)
    }

    fn sweep_idle(&self) {
        let now_ms = now_ms();
        let cutoff = now_ms.saturating_sub(BUCKET_IDLE_TTL_MS);
        let mut buckets = self.buckets.lock().expect("per-ip bucket map poisoned");
        buckets.retain(|_, b| b.last_refill_ms_for_gc() >= cutoff);
    }

    /// Number of live buckets, for observability + tests.
    pub fn bucket_count(&self) -> usize {
        self.buckets
            .lock()
            .expect("per-ip bucket map poisoned")
            .len()
    }
}

/// Wall-clock unix millis. The mempool's bucket uses the same time
/// source; routing both through one helper keeps drift impossible.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Axum middleware: gate every inbound request on the per-IP bucket.
///
/// Wired via `axum::middleware::from_fn_with_state` so the layer can
/// extract `ConnectInfo<SocketAddr>` from request extensions. If the
/// extension is missing (e.g. unit tests that don't go through
/// `into_make_service_with_connect_info`), the request is admitted —
/// rate limiting is a defense-in-depth layer, not a correctness
/// invariant, and unit tests should not have to fake socket addrs.
///
/// When the node sits behind an ALB or reverse proxy, the TCP source
/// IP is the proxy's address — all clients share one bucket. To fix
/// this, the middleware checks `X-Forwarded-For` first (standard
/// header injected by AWS ALB, nginx, etc.) and falls back to
/// `ConnectInfo` for direct connections. Only the leftmost (client)
/// IP in the `X-Forwarded-For` chain is used; intermediate proxies
/// are ignored.
pub async fn middleware(
    State(limiter): State<Arc<PerIpRateLimiter>>,
    req: Request,
    next: Next,
) -> Response {
    // Resolve the effective client IP. Trust X-Forwarded-For ONLY
    // when the TCP source is a known proxy (ALB, nginx, etc.).
    // Direct clients forging X-Forwarded-For are ignored — they
    // get rate-limited by their real TCP source IP.
    let tcp_ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip());

    let ip = if let Some(tcp) = tcp_ip {
        if limiter.trusted_proxy_ips.contains(&tcp) {
            // TCP source is a trusted proxy — parse the forwarded
            // client IP (leftmost in X-Forwarded-For chain).
            req.headers()
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.split(',').next())
                .and_then(|s| s.trim().parse::<IpAddr>().ok())
                .unwrap_or(tcp)
        } else {
            tcp
        }
    } else {
        // No ConnectInfo (unit tests) — admit without rate limiting.
        return next.run(req).await;
    };

    if limiter.try_acquire(ip, now_ms()).is_ok() {
        return next.run(req).await;
    }

    let id = peek_jsonrpc_id(req).await;
    let body = JsonRpcResponse::err(
        id,
        RpcError::RateLimited(format!("per-IP rate limit exceeded for {ip}")),
    );
    (StatusCode::OK, Json(body)).into_response()
}

/// Best-effort extraction of the `id` field from a buffered JSON-RPC
/// request body. We have to consume the body to read it, but at this
/// point the request is rejected anyway — the inner handler will
/// never run, so swallowing the bytes is fine. Cap at 64 KiB to
/// bound the work on a rejected request (the upstream
/// `RequestBodyLimitLayer` already enforces 1 MiB; this is a tighter
/// internal cap for the rejection path so a giant body doesn't
/// inflate the per-IP rejection cost).
async fn peek_jsonrpc_id(req: Request) -> Value {
    let (_, body) = req.into_parts();
    let bytes = match to_bytes(body, 64 * 1024).await {
        Ok(b) => b,
        Err(_) => return Value::Null,
    };
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(mut map)) => map.remove("id").unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bucket_admits_burst_up_to_capacity() {
        let limiter = PerIpRateLimiter::new(5, 1);
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        for _ in 0..5 {
            assert!(limiter.try_acquire(ip, 1_000).is_ok());
        }
        assert!(limiter.try_acquire(ip, 1_000).is_err());
    }

    #[test]
    fn x_forwarded_for_parsed_correctly() {
        // Simulate the header parsing logic from the middleware.
        let parse = |header: &str| -> Option<IpAddr> {
            header
                .split(',')
                .next()
                .and_then(|s| s.trim().parse::<IpAddr>().ok())
        };
        // Single IP.
        assert_eq!(parse("203.0.113.50"), Some("203.0.113.50".parse().unwrap()));
        // Multiple IPs — leftmost is the client.
        assert_eq!(
            parse("203.0.113.50, 70.41.3.18, 150.172.238.178"),
            Some("203.0.113.50".parse().unwrap())
        );
        // Whitespace around the IP.
        assert_eq!(
            parse("  10.0.0.1 , 10.0.0.2"),
            Some("10.0.0.1".parse().unwrap())
        );
        // Garbage — returns None, middleware falls back to ConnectInfo.
        assert_eq!(parse("not-an-ip, 10.0.0.1"), None);
        // Empty string.
        assert_eq!(parse(""), None);
    }

    #[tokio::test]
    async fn buckets_are_independent_across_ips() {
        let limiter = PerIpRateLimiter::new(2, 1);
        let a: IpAddr = "10.0.0.1".parse().unwrap();
        let b: IpAddr = "10.0.0.2".parse().unwrap();
        assert!(limiter.try_acquire(a, 1_000).is_ok());
        assert!(limiter.try_acquire(a, 1_000).is_ok());
        assert!(limiter.try_acquire(a, 1_000).is_err());
        // B is untouched.
        assert!(limiter.try_acquire(b, 1_000).is_ok());
        assert!(limiter.try_acquire(b, 1_000).is_ok());
        assert!(limiter.try_acquire(b, 1_000).is_err());
        assert_eq!(limiter.bucket_count(), 2);
    }
}
