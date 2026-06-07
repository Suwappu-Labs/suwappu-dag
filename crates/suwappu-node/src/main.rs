//! suwappu-node — top-level Suwappu DAG node binary.
//!
//! Loads a TOML config + shared genesis manifest, starts the validator
//! daemon, and runs until killed. The daemon composes wire transport, DAG
//! consensus, joint-quorum voting, block execution, and the client intent
//! listener.
//!
//! Usage:
//!
//! ```text
//! suwappu-node --config /etc/suwappu/node.toml
//! ```

use anyhow::{Context, Result};
use clap::Parser;
use suwappu_node::{Daemon, GenesisManifest, NodeConfig};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "suwappu-node", version, about = "Suwappu DAG validator daemon")]
struct Args {
    /// Path to the per-validator TOML config.
    #[arg(long)]
    config: std::path::PathBuf,
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let args = Args::parse();

    let cfg = NodeConfig::from_path(&args.config)
        .with_context(|| format!("load node config {}", args.config.display()))?;
    let manifest = GenesisManifest::from_path(&cfg.genesis_manifest_path)
        .with_context(|| format!("load genesis {}", cfg.genesis_manifest_path.display()))?;

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        self_id = %cfg.self_id,
        authority_id = cfg.authority_id,
        listen = %cfg.listen,
        client_listen = %cfg.client_listen,
        peers = cfg.peers.len(),
        "suwappu-node starting"
    );

    let _daemon = Daemon::start(cfg, manifest).await?;
    tracing::info!("suwappu-node running — SIGINT/SIGTERM to stop");

    // Park until the process is signalled; tokio's ctrl_c also catches
    // SIGTERM from systemd on Linux.
    tokio::signal::ctrl_c().await.ok();
    tracing::info!("suwappu-node shutting down");
    Ok(())
}
