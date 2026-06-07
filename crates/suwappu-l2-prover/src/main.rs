//! suwappu-l2-prover CLI entry point (Track G G4.1, #104).
//!
//! Loads config, initializes tracing, starts the metrics server +
//! poll loop, and parks until Ctrl-C. The poll loop runs the gated
//! [`StubProver`] — deploying this binary today produces no proofs,
//! only a clear log line explaining the #88/#94 gate. The real prover
//! lands once those gates clear.

use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use clap::Parser;
use suwappu_l2_prover::{
    metrics_http,
    poll_loop::run_loop,
    prover::{mock::MockBatchSource, RetryPolicy, StubProver},
    Metrics, ProverConfig,
};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "suwappu-l2-prover",
    version,
    about = "SUWAPPU L2 prover daemon (Track G G4.1, #104)"
)]
struct Cli {
    /// Path to the TOML configuration file.
    #[arg(long, value_name = "PATH")]
    config: PathBuf,
}

fn main() -> Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,suwappu_l2_prover=debug"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let cli = Cli::parse();
    let cfg = ProverConfig::load_from_path(&cli.config)
        .with_context(|| format!("loading config from {}", cli.config.display()))?;

    let mut rt = tokio::runtime::Builder::new_multi_thread();
    rt.enable_all();
    if cfg.tokio_worker_threads > 0 {
        rt.worker_threads(cfg.tokio_worker_threads);
    }
    let runtime = rt.build().context("building tokio runtime")?;

    runtime.block_on(run(cfg))
}

async fn run(cfg: ProverConfig) -> Result<()> {
    let metrics_addr: SocketAddr = cfg
        .metrics_bind_addr
        .parse()
        .with_context(|| format!("parsing metrics_bind_addr {}", cfg.metrics_bind_addr))?;

    info!(
        sequencer_rpc_url = %cfg.sequencer_rpc_url,
        metrics_bind_addr = %cfg.metrics_bind_addr,
        poll_interval_ms = cfg.poll_interval_ms,
        max_attempts = cfg.max_attempts,
        "suwappu-l2-prover: starting"
    );
    warn!(
        "suwappu-l2-prover: SP1 proving is GATED (StubProver) — no proofs will be produced until \
         #88 (revm-in-guest) and #94 (sp1-sdk decision) land. See suwappu_l2_prover::prover."
    );

    let metrics = Arc::new(Metrics::new());
    let metrics_handle = metrics_http::serve(metrics_addr, metrics.clone())
        .await
        .context("starting metrics server")?;

    let policy = RetryPolicy {
        max_attempts: cfg.max_attempts,
        base_backoff: Duration::from_millis(cfg.backoff_base_ms),
        max_backoff: Duration::from_millis(cfg.backoff_max_ms),
    };

    // The real source is a JSON-RPC client to the sequencer daemon
    // (`suwappu_getUnprovenBatch` / `suwappu_submitIntent`); that wiring lands
    // in a follow-up. The mock keeps the binary bootable + the loop
    // exercising the gated StubProver end-to-end until then.
    let source = Arc::new(MockBatchSource::new());
    let prover = Arc::new(StubProver::new());
    info!("suwappu-l2-prover: source = mock (real sequencer JSON-RPC client lands in a follow-up)");

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown = Box::pin(async move {
        let _ = shutdown_rx.await;
    });

    let loop_handle = tokio::spawn(run_loop(
        source,
        prover,
        policy,
        metrics,
        Duration::from_millis(cfg.poll_interval_ms),
        shutdown,
    ));

    tokio::signal::ctrl_c()
        .await
        .context("installing ctrl-c handler")?;
    info!("suwappu-l2-prover: ctrl-c received, shutting down");
    let _ = shutdown_tx.send(());
    loop_handle.await.context("poll loop task join")?;
    metrics_handle.abort();
    Ok(())
}
