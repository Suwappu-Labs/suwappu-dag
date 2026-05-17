//! `gsx-validator-program` binary.
//!
//! Spawns probe + scoring tasks, serves the leaderboard HTTP API +
//! the foundation-admin endpoints.

use std::net::SocketAddr;

use axum::{
    routing::{get, post},
    Router,
};
use clap::Parser;
use gsx_validator_program::{
    admin::{
        handle_award, handle_list_awards, handle_list_operators, handle_record_certs,
        handle_register_operator, AdminState,
    },
    init_db,
    leaderboard::handle_leaderboard,
    probe, score,
};
use sqlx::postgres::PgPoolOptions;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug, Clone)]
#[command(
    name = "gsx-validator-program",
    about = "Testnet points-accumulator daemon"
)]
struct Args {
    /// Postgres connection string. Recommended: read from
    /// AWS Secrets Manager + pass via env at systemd unit
    /// start.
    #[arg(long, env = "GSX_PROGRAM_DATABASE_URL")]
    database_url: String,

    /// Public RPC URL probed for uptime. Defaults to the public
    /// testnet ALB.
    #[arg(
        long,
        default_value = "https://rpc.testnet.gsx.globalsettlement.com",
        env = "GSX_PROGRAM_RPC_URL"
    )]
    rpc_url: String,

    /// HTTP bind address. Behind a Route53 A record for
    /// `program.testnet.gsx.globalsettlement.com`.
    #[arg(long, default_value = "0.0.0.0:8090", env = "GSX_PROGRAM_BIND")]
    bind: SocketAddr,

    /// Bearer token gating the `/admin/*` endpoints. Foundation-
    /// supplied; rotate via Secrets Manager + systemd restart.
    #[arg(long, env = "GSX_PROGRAM_ADMIN_TOKEN")]
    admin_token: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new("gsx_validator_program=info,axum=warn,tower_http=info,sqlx=warn")
        }))
        .init();

    let args = Args::parse();

    info!(
        rpc_url = %args.rpc_url,
        bind = %args.bind,
        "gsx-validator-program: starting"
    );

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&args.database_url)
        .await?;

    init_db(&pool).await?;
    info!("gsx-validator-program: schema migrated");

    // Probe + scoring tasks run forever in the background.
    let probe_pool = pool.clone();
    let probe_rpc_url = args.rpc_url.clone();
    tokio::spawn(async move {
        probe::run_probe_loop(probe_pool, probe_rpc_url).await;
    });

    let scoring_pool = pool.clone();
    tokio::spawn(async move {
        score::run_scoring_loop(scoring_pool).await;
    });

    let admin_state = AdminState {
        pool: pool.clone(),
        admin_token: args.admin_token,
    };

    let app = Router::new()
        // Public read.
        .route("/leaderboard", get(handle_leaderboard))
        .route("/health", get(handle_health))
        .with_state(pool.clone())
        .merge(
            Router::new()
                .route("/admin/operators", post(handle_register_operator))
                .route("/admin/operators", get(handle_list_operators))
                .route("/admin/award", post(handle_award))
                .route("/admin/certs", post(handle_record_certs))
                .route(
                    "/admin/awards/:authority_id",
                    get(handle_list_awards),
                )
                .with_state(admin_state),
        );
    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    let actual = listener.local_addr()?;
    info!(addr = %actual, "gsx-validator-program: HTTP server bound");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn handle_health() -> &'static str {
    "ok"
}
