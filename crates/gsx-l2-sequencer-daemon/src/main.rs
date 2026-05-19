//! gsx-l2-sequencer-daemon CLI entry point.
//!
//! Phase 2.2-a scaffold: loads config, initializes tracing,
//! starts the Tokio runtime, logs a "ready" line, and parks.
//! The batch-builder + force-include watcher + JSON-RPC server
//! tasks land in follow-up commits (Phase 2.2-b/c/d).

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use gsx_l2_sequencer_daemon::SequencerConfig;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "gsx-l2-sequencer-daemon",
    version,
    about = "GSX L2 sequencer daemon (Track G G4.2, #105)"
)]
struct Cli {
    /// Path to the TOML configuration file.
    #[arg(long, value_name = "PATH")]
    config: PathBuf,
}

fn main() -> Result<()> {
    // RUST_LOG controls the verbosity; default to info if unset.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,gsx_l2_sequencer_daemon=debug"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let cli = Cli::parse();
    let cfg = SequencerConfig::load_from_path(&cli.config)
        .with_context(|| format!("loading config from {}", cli.config.display()))?;

    let mut rt = tokio::runtime::Builder::new_multi_thread();
    rt.enable_all();
    if cfg.tokio_worker_threads > 0 {
        rt.worker_threads(cfg.tokio_worker_threads);
    }
    let runtime = rt.build().context("building tokio runtime")?;

    runtime.block_on(run(cfg))
}

async fn run(cfg: SequencerConfig) -> Result<()> {
    info!(
        l1_rpc_url = %cfg.l1_rpc_url,
        l2_chain_id = %cfg.l2_chain_id,
        rpc_bind_addr = %cfg.rpc_bind_addr,
        batch_interval_ms = cfg.batch_interval_ms,
        "gsx-l2-sequencer-daemon: scaffold up, awaiting follow-up Phase 2.2 tasks"
    );

    // Phase 2.2-b/c/d will spawn the real tasks here. For now,
    // park so deployment harnesses can validate the binary
    // boots + holds the port (once 2.2-d adds RPC server).
    futures_park().await;
    Ok(())
}

/// Park indefinitely. Cleaner than `loop { sleep(forever) }`
/// because it doesn't burn a Tokio timer slot.
async fn futures_park() {
    // `std::future::pending` resolves never. The runtime keeps
    // running other tasks; this one just never wakes.
    std::future::pending::<()>().await
}
