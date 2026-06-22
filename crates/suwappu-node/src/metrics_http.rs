//! G6: Prometheus text-format metrics endpoint.
//!
//! Minimal, dependency-free exporter served on the operator-configured
//! `metrics_listen` socket. Designed to be scraped by a local
//! `amazon-cloudwatch-agent` instance (or any Prometheus-compatible
//! scraper) on the same host — bound to `127.0.0.1` by default so the
//! security group never has to open this port.
//!
//! ## Why not a Prometheus crate?
//!
//! `prometheus` and `prometheus-client` are both reasonable; both add
//! ~20 transitive deps. The text format itself is dead simple
//! (`# TYPE name kind` + `name{labels} value`), and the v0.1 metric
//! set is short (~10 metrics). Hand-rolling keeps the binary lean
//! and the dep tree easy to audit — important for a long-running
//! validator process.
//!
//! ## Metric set
//!
//! Read directly from the existing `State` snapshot. NO hot-path
//! instrumentation surgery in this pass — that risks subtle
//! consensus regressions for marginal monitoring value. Fine-grained
//! per-method / per-peer counters land in a follow-up if the halt
//! and silent-peer alarms demand more dimensions.
//!
//! - `suwappu_last_committed_round` (gauge) — `inner.blocks_by_round`
//!   max key. Halt alarm's primary signal.
//! - `suwappu_committed_rounds_total` (counter) — `state.committed.len()`.
//! - `suwappu_mempool_size` (gauge) — `state.mempool.stats().queued`.
//! - `suwappu_node_info{authority_id, region}` (gauge, always 1) — labels
//!   only; identifies this validator.
//! - `suwappu_process_uptime_seconds` (gauge) — seconds since process
//!   start. Reset signal for the silent-peer alarm.

use std::{
    fmt::Write,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Instant,
};

use axum::{extract::State, response::IntoResponse, routing::get, Router};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::daemon::State as NodeState;

/// Static labels rendered on `suwappu_node_info` so dashboards can
/// pivot on identity without parsing the rest of the metric names.
#[derive(Clone)]
pub struct NodeIdentity {
    /// Matches `NodeConfig::self_id`.
    pub region: String,
    /// Matches `NodeConfig::authority_id`.
    pub authority_id: u32,
}

/// Per-process counters that DON'T read directly from `State`.
/// Kept tiny on purpose; fine-grained counters land in a follow-up.
pub struct MetricsCounters {
    /// Process startup `Instant`. Subtracted on each scrape to
    /// produce the `suwappu_process_uptime_seconds` gauge.
    started_at: Instant,
    /// Total scrape count. Useful for "is the scraper alive?"
    /// dashboards.
    scrapes_total: AtomicU64,
}

impl Default for MetricsCounters {
    fn default() -> Self {
        Self {
            started_at: Instant::now(),
            scrapes_total: AtomicU64::new(0),
        }
    }
}

/// Combined state pointer the axum handler reads on each scrape.
#[derive(Clone)]
struct MetricsState {
    node: Arc<NodeState>,
    identity: NodeIdentity,
    counters: Arc<MetricsCounters>,
}

/// Spawn the metrics HTTP server. Returns the join handle for the
/// listener task — drop it (or let the daemon's task list drain)
/// to stop.
///
/// Returns `Ok(None)` if `addr` is `None` (operator didn't configure
/// the endpoint).
///
/// `pub(crate)` because the `NodeState` argument is `pub(crate)`;
/// only `daemon.rs` is a caller in practice.
pub(crate) async fn start_if_configured(
    addr: Option<SocketAddr>,
    node: Arc<NodeState>,
    identity: NodeIdentity,
) -> anyhow::Result<Option<JoinHandle<()>>> {
    let Some(addr) = addr else {
        info!("suwappu-metrics: metrics_listen not set; endpoint disabled");
        return Ok(None);
    };

    let counters = Arc::new(MetricsCounters::default());
    let state = MetricsState {
        node,
        identity,
        counters,
    };

    let app = Router::new()
        .route("/metrics", get(render_metrics))
        .route("/health", get(render_health))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    let actual = listener.local_addr()?;
    info!(addr = %actual, "suwappu-metrics: bound");

    let handle = tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, app).await {
            warn!(error = %err, "suwappu-metrics: server exited");
        }
    });
    Ok(Some(handle))
}

async fn render_health() -> impl IntoResponse {
    // Liveness: as long as the metrics task is responding, the
    // validator process is alive enough to answer HTTP. Doesn't
    // imply consensus liveness — that's the halt alarm's job
    // (suwappu_last_committed_round flat → page).
    "ok"
}

async fn render_metrics(State(state): State<MetricsState>) -> impl IntoResponse {
    state.counters.scrapes_total.fetch_add(1, Ordering::Relaxed);

    // Snapshot every field BEFORE taking the .await on inner so we
    // never hold a lock guard across an await point. `committed` +
    // `blocks` are parking_lot (sync) so they can be queried after.
    let last_committed_round = {
        let inner = state.node.inner.lock().await;
        inner
            .blocks_by_round
            .keys()
            .next_back()
            .copied()
            .unwrap_or(0)
    };

    let (committed_total, mempool_size) = {
        let committed = state.node.committed.lock();
        let mempool_stats = state.node.mempool.stats();
        (committed.len() as u64, mempool_stats.size as u64)
    };

    let uptime_secs = state.counters.started_at.elapsed().as_secs();
    let scrapes_total = state.counters.scrapes_total.load(Ordering::Relaxed);

    let mut out = String::with_capacity(1024);

    // suwappu_node_info — single-sample identity gauge so dashboards
    // can pivot on (region, authority_id) without parsing.
    let _ = writeln!(
        out,
        "# HELP suwappu_node_info Static labels identifying this validator."
    );
    let _ = writeln!(out, "# TYPE suwappu_node_info gauge");
    let _ = writeln!(
        out,
        "suwappu_node_info{{region=\"{}\",authority_id=\"{}\"}} 1",
        escape_label(&state.identity.region),
        state.identity.authority_id
    );

    let _ = writeln!(out, "# HELP suwappu_last_committed_round Highest committed round seen by this validator. Halt-alarm signal: flat for >5 min → cluster has stopped progressing.");
    let _ = writeln!(out, "# TYPE suwappu_last_committed_round gauge");
    let _ = writeln!(out, "suwappu_last_committed_round {last_committed_round}");

    let _ = writeln!(out, "# HELP suwappu_committed_rounds_total Number of distinct cert hashes this validator has marked committed.");
    let _ = writeln!(out, "# TYPE suwappu_committed_rounds_total counter");
    let _ = writeln!(out, "suwappu_committed_rounds_total {committed_total}");

    let _ = writeln!(
        out,
        "# HELP suwappu_mempool_size Current queued-intent count in this validator's mempool."
    );
    let _ = writeln!(out, "# TYPE suwappu_mempool_size gauge");
    let _ = writeln!(out, "suwappu_mempool_size {mempool_size}");

    let _ = writeln!(
        out,
        "# HELP suwappu_process_uptime_seconds Seconds since the validator process started."
    );
    let _ = writeln!(out, "# TYPE suwappu_process_uptime_seconds gauge");
    let _ = writeln!(out, "suwappu_process_uptime_seconds {uptime_secs}");

    let _ = writeln!(out, "# HELP suwappu_metrics_scrapes_total Cumulative count of /metrics scrapes this process has served.");
    let _ = writeln!(out, "# TYPE suwappu_metrics_scrapes_total counter");
    let _ = writeln!(out, "suwappu_metrics_scrapes_total {scrapes_total}");

    (
        axum::http::StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4")],
        out,
    )
}

/// Escape a label value per Prometheus exposition format rules:
/// `"`, `\`, `\n` must be escaped. Region labels are operator-
/// supplied so we don't trust them to be already clean.
fn escape_label(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str(r"\\"),
            '"' => out.push_str(r#"\""#),
            '\n' => out.push_str(r"\n"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_escape_handles_special_chars() {
        assert_eq!(escape_label("us-east-1"), "us-east-1");
        assert_eq!(escape_label(r#"weird"region"#), r#"weird\"region"#);
        assert_eq!(escape_label("multi\nline"), "multi\\nline");
        assert_eq!(escape_label(r"back\slash"), r"back\\slash");
    }
}
