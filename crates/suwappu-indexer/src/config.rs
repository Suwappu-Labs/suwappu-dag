//! Runtime configuration for `suwappu-indexer`.

use std::net::SocketAddr;

use clap::Parser;

/// Indexer runtime configuration.
#[derive(Debug, Clone, Parser)]
#[command(name = "suwappu-indexer", about = "Streaming indexer for suwappu-dag")]
pub struct IndexerConfig {
    /// WebSocket URL of the suwappu-rpc server to subscribe to.
    /// Example: `ws://127.0.0.1:9092/ws`.
    #[arg(long, env = "SUWAPPU_INDEXER_WS_URL")]
    pub ws_url: String,

    /// HTTP URL of the suwappu-rpc server. Used for catch-up backfill
    /// (querying `suwappu_getBlock` from the last checkpoint to fill any
    /// gap before the WebSocket subscription took hold).
    ///
    /// MVP doesn't use this yet — backfill lands in T7 follow-up.
    #[arg(long, env = "SUWAPPU_INDEXER_RPC_URL")]
    pub rpc_url: Option<String>,

    /// Local TCP socket to bind for the indexer's HTTP read API.
    /// Example: `0.0.0.0:9093`.
    #[arg(long, default_value = "127.0.0.1:9093", env = "SUWAPPU_INDEXER_LISTEN")]
    pub listen: SocketAddr,

    /// Reconnect delay (seconds) on WebSocket disconnect. The
    /// subscriber retries with this gap; on `Lagged` notices it
    /// reconnects immediately.
    #[arg(long, default_value_t = 5, env = "SUWAPPU_INDEXER_RECONNECT_SECS")]
    pub reconnect_secs: u64,

    /// Optional Postgres URL for persistent storage. When set (and
    /// the binary was built with `--features postgres`), the indexer
    /// uses `PostgresStore` instead of the in-memory store and
    /// applies the embedded migrations on startup. Example:
    /// `postgres://suwappu:suwappu@127.0.0.1:5432/suwappu_indexer`.
    ///
    /// When unset OR when the binary was built without the
    /// `postgres` feature, the in-memory store is used (state is
    /// lost on restart).
    #[arg(long, env = "SUWAPPU_INDEXER_DATABASE_URL")]
    pub database_url: Option<String>,
}
