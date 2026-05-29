//! `/metrics` HTTP server for the prover daemon (Track G G4.1, #104).
//!
//! Thin axum wrapper around [`Metrics::render`]. Bound to
//! `127.0.0.1` by default (see `ProverConfig::metrics_bind_addr`) so
//! the security group never opens the port — scraped by a local
//! cloudwatch-agent. Mirrors `gsx-node`'s `metrics_http.rs`.

use std::{net::SocketAddr, sync::Arc};

use axum::{extract::State, response::IntoResponse, routing::get, Router};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::metrics::Metrics;

/// Spawn the metrics HTTP server bound to `addr`. Returns the listener
/// task's join handle; drop it to stop serving.
pub async fn serve(addr: SocketAddr, metrics: Arc<Metrics>) -> anyhow::Result<JoinHandle<()>> {
    let app = Router::new()
        .route("/metrics", get(render_metrics))
        .route("/health", get(render_health))
        .with_state(metrics);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    let actual = listener.local_addr()?;
    info!(addr = %actual, "gsx-l2-prover metrics: bound");

    let handle = tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, app).await {
            warn!(error = %err, "gsx-l2-prover metrics: server exited");
        }
    });
    Ok(handle)
}

async fn render_health() -> impl IntoResponse {
    // Liveness only: the process is answering HTTP. Proving liveness
    // is the `batches_proven_total` / `prove_failures_total` alarm's
    // job, not this endpoint's.
    "ok"
}

async fn render_metrics(State(metrics): State<Arc<Metrics>>) -> impl IntoResponse {
    (
        axum::http::StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4")],
        metrics.render(),
    )
}
