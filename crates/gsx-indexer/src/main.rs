//! `gsx-indexer` binary.

use std::sync::Arc;

use clap::Parser;
use gsx_indexer::{
    api,
    config::IndexerConfig,
    store::{InMemoryStore, Store},
    subscriber,
};
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

    // Runtime-pick the storage backend. When `--database-url` is set
    // AND the binary was compiled with `--features postgres`, use
    // `PostgresStore`; otherwise fall back to in-memory.
    if let Some(database_url) = &cfg.database_url {
        #[cfg(feature = "postgres")]
        {
            info!("gsx-indexer: connecting to Postgres + applying migrations",);
            let store = gsx_indexer::PostgresStore::connect(database_url).await?;
            let latest = store.latest_round().await;
            match latest {
                Some(r) => info!(
                    latest_round = r,
                    "gsx-indexer: resuming from persisted state — WS tail \
                     starts at live; commits between (latest_round+1, head] \
                     are missed until backfill via gsx_getBlock lands"
                ),
                None => info!(
                    "gsx-indexer: persistent store is empty; starting from \
                     live WS tail"
                ),
            }
            return run(cfg, store).await;
        }
        #[cfg(not(feature = "postgres"))]
        {
            let _ = database_url;
            anyhow::bail!(
                "--database-url was set but this binary was built without \
                 the `postgres` feature. Rebuild with \
                 `cargo build -p gsx-indexer --features postgres`, or \
                 clear --database-url to fall back to in-memory storage.",
            );
        }
    } else {
        info!(
            "gsx-indexer: using in-memory store (state lost on restart). \
             Pass --database-url + build with --features postgres for \
             persistent storage."
        );
        run(cfg, InMemoryStore::new()).await
    }
}

/// Monomorphized post-config startup over the chosen store backend.
/// `S: Store + Clone + 'static` so both the subscriber task and the
/// axum router can share an owned + cloned reference.
async fn run<S>(cfg: IndexerConfig, store: S) -> anyhow::Result<()>
where
    S: Store + Clone + 'static,
{
    let store = Arc::new(store);

    // F2 (A5 Phase 2): catch-up backfill before the live tail
    // takes over, so the store is reconciled with the chain head
    // even if the indexer was offline through some commits. Skipped
    // when `rpc_url` is unset — operators can opt out by leaving the
    // CLI flag off (the live tail then starts from whatever the WS
    // emits, with any pre-startup gap going unfilled).
    if let Some(rpc_url) = cfg.rpc_url.clone() {
        info!(rpc_url = %rpc_url, "gsx-indexer: running startup backfill");
        if let Err(e) = gsx_indexer::backfill::catch_up(
            &*store,
            &rpc_url,
            gsx_indexer::backfill::DEFAULT_MAX_PER_ITER,
        )
        .await
        {
            warn!(error = %e, "gsx-indexer: backfill failed — continuing with live tail only");
        }
    } else {
        info!(
            "gsx-indexer: --rpc-url not set; skipping startup backfill. \
             Pre-startup gaps will not be filled."
        );
    }

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
