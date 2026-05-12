//! gsx-node — top-level GSX DAG node binary.
//!
//! Composes the consensus, authority/validator, fast-path, execution, precompile,
//! LTP, and transport crates into a single running validator. Configuration is
//! read from `config.toml`; telemetry is emitted via `tracing`.
//!
//! Phase-1 binary is a no-op shell — actual subsystem wiring lands sprint by
//! sprint per `docs/architecture/sprint-map.md`.

use anyhow::Result;
use tracing_subscriber::EnvFilter;

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "gsx-node starting (phase-1 shell — subsystem wiring lands per sprint)",
    );
    Ok(())
}
