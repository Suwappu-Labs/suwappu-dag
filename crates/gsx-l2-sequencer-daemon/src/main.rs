//! gsx-l2-sequencer-daemon CLI entry point.
//!
//! Phase 2.2-a scaffold: loads config, initializes tracing,
//! starts the Tokio runtime, logs a "ready" line, and parks.
//! The batch-builder + force-include watcher + JSON-RPC server
//! tasks land in follow-up commits (Phase 2.2-b/c/d).

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result};
use clap::Parser;
use gsx_l2_sequencer_daemon::{
    batch_builder_task, l1_client::mock::MockL1Client, BatchBuilderTaskConfig, SequencerConfig,
    SequencerState,
};
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

/// Redact an L1 RPC URL for logging (#244).
///
/// The configured `l1_rpc_url` may embed credentials
/// (`https://user:pass@host`) or an API token in the path/query
/// (`https://host/v2/<token>`). Logging it verbatim at startup
/// leaks secrets into log aggregation. This extracts the host
/// (and `:port` if present) only — never userinfo, path, or
/// query. No `url` crate is available in this crate's deps, so
/// this is a small hand-rolled extractor.
fn redact_l1_rpc_url(raw: &str) -> String {
    // Strip the scheme (`https://`, `wss://`, ...).
    let after_scheme = raw.split_once("://").map(|(_, rest)| rest).unwrap_or(raw);
    // Drop everything from the first path/query/fragment
    // delimiter onward — a token can hide in any of them.
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme);
    // Drop userinfo (`user:pass@`). Use the LAST `@` so a stray
    // `@` in credentials can't smuggle host bytes through.
    let host_port = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    if host_port.is_empty() {
        "<redacted>".to_string()
    } else {
        host_port.to_string()
    }
}

async fn run(cfg: SequencerConfig) -> Result<()> {
    let l2_chain_id_hash = BatchBuilderTaskConfig::derive_l2_chain_id_hash(&cfg.l2_chain_id);
    info!(
        l1_rpc_host = %redact_l1_rpc_url(&cfg.l1_rpc_url),
        l2_chain_id = %cfg.l2_chain_id,
        l2_chain_id_hash = ?l2_chain_id_hash,
        rpc_bind_addr = %cfg.rpc_bind_addr,
        batch_interval_ms = cfg.batch_interval_ms,
        "gsx-l2-sequencer-daemon: starting"
    );

    let state = Arc::new(Mutex::new(SequencerState::new()));

    // Phase 2.2-c will replace the mock with the real
    // gsx-client-backed L1Client. The mock keeps the binary
    // bootable + the batch-builder loop testable end-to-end
    // until then.
    let l1: Arc<MockL1Client> = Arc::new(MockL1Client::new());
    info!("l1 client: mock (real gsx-client wiring lands in Phase 2.2-c)");

    let builder_cfg = BatchBuilderTaskConfig {
        interval: Duration::from_millis(cfg.batch_interval_ms),
        l2_chain_id_hash,
        range_vk_commitment: [0u8; 32],
    };

    // Ctrl-C triggers the shutdown future. Tasks observe it
    // via the oneshot and exit cleanly.
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown = Box::pin(async move {
        let _ = shutdown_rx.await;
    });

    let builder_handle = tokio::spawn(batch_builder_task::run_loop(
        state.clone(),
        builder_cfg,
        l1.clone(),
        shutdown,
    ));

    tokio::signal::ctrl_c()
        .await
        .context("installing ctrl-c handler")?;
    info!("ctrl-c received, shutting down");
    let _ = shutdown_tx.send(());
    builder_handle.await.context("batch builder task join")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::redact_l1_rpc_url;

    #[test]
    fn redact_strips_userinfo_credentials() {
        // user:pass@ must never appear in the redacted output.
        let out = redact_l1_rpc_url("https://alice:s3cret@rpc.gsx.example.com:8545/v2");
        assert_eq!(out, "rpc.gsx.example.com:8545");
        assert!(!out.contains("alice"));
        assert!(!out.contains("s3cret"));
        assert!(!out.contains('@'));
    }

    #[test]
    fn redact_strips_path_query_token() {
        // A token hiding in the path or query must be dropped.
        let out = redact_l1_rpc_url("https://rpc.example.com/v2/SUPERSECRETTOKEN?key=abc123");
        assert_eq!(out, "rpc.example.com");
        assert!(!out.contains("SUPERSECRETTOKEN"));
        assert!(!out.contains("abc123"));
    }

    #[test]
    fn redact_keeps_plain_host_and_port() {
        assert_eq!(
            redact_l1_rpc_url("https://rpc.testnet.gsx.globalsettlement.com"),
            "rpc.testnet.gsx.globalsettlement.com"
        );
        assert_eq!(redact_l1_rpc_url("http://127.0.0.1:8545"), "127.0.0.1:8545");
    }

    #[test]
    fn redact_handles_missing_scheme_and_empty() {
        assert_eq!(
            redact_l1_rpc_url("rpc.example.com:8545"),
            "rpc.example.com:8545"
        );
        assert_eq!(redact_l1_rpc_url(""), "<redacted>");
    }
}
