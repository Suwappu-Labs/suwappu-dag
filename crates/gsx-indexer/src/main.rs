//! `gsx-indexer` binary.

use std::sync::Arc;

use clap::Parser;
use gsx_indexer::{api, config::IndexerConfig, store::InMemoryStore, subscriber};
use tracing::{info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "gsx_indexer=info,tower_http=warn".into()),
        )
        .init();

    let cfg = IndexerConfig::parse();
    info!(?cfg, "gsx-indexer: starting");

    let store = Arc::new(InMemoryStore::new());

    // Subscriber task — runs forever, reconnects on disconnect.
    {
        let store = store.clone();
        let url = cfg.ws_url.clone();
        let backoff = cfg.reconnect_secs;
        tokio::spawn(async move {
            subscriber::run(url, (*store).clone(), backoff).await;
        });
    }

    // HTTP read API.
    let app = api::router(store.clone());
    let listener = tokio::net::TcpListener::bind(cfg.listen).await?;
    let actual = listener.local_addr()?;
    info!(addr = %actual, "gsx-indexer: HTTP API bound");
    if let Err(e) = axum::serve(listener, app).await {
        warn!(error = %e, "gsx-indexer: HTTP server exited");
    }
    Ok(())
}
