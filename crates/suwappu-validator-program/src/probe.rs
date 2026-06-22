//! Uptime probe loop.
//!
//! Every `PROBE_INTERVAL` (60s), `suwappu_getEpoch` is called against
//! a configured RPC URL — for v1, the ALB-fronted public RPC at
//! `https://rpc.testnet.suwappu.bot`. Success or
//! failure is recorded per-authority in `uptime_samples`.
//!
//! ## Why probe the ALB instead of each validator directly?
//!
//! v1: the foundation just probes the cluster as a whole. The
//! ALB load-balances across all 7 seeds, so a single ALB probe
//! tells us "is the cluster serving traffic." Per-validator
//! probing would require dialing each seed's EIP individually +
//! every external operator's public address — substantially
//! more infrastructure (DNS for each operator, etc.).
//!
//! Per-operator probing is in v2 alongside the S3 NDJSON
//! ingest. For now, **all operators get a shared uptime signal**
//! based on the cluster-level probe. Real per-operator
//! attribution lands when we wire up the cert-observation
//! ingest from the operator's `events.ndjson` uploads.

use std::time::Instant;

use suwappu_client::Client;
use sqlx::PgPool;
use tracing::{debug, warn};

use crate::PROBE_INTERVAL;

/// Run the probe loop forever. Exits only on a fatal Postgres
/// error.
pub async fn run_probe_loop(pool: PgPool, rpc_url: String) {
    let client = Client::new(rpc_url.clone());
    let mut tick = tokio::time::interval(PROBE_INTERVAL);
    loop {
        tick.tick().await;
        if let Err(e) = probe_once(&pool, &client).await {
            warn!(error = %e, rpc_url = %rpc_url, "probe: tick failed");
        }
    }
}

async fn probe_once(pool: &PgPool, client: &Client) -> Result<(), crate::ProgramError> {
    let started = Instant::now();
    let probe_result = client.get_epoch().await;
    let latency_ms = started.elapsed().as_millis() as i32;

    let ok = match probe_result {
        Ok(_) => true,
        Err(e) => {
            debug!(error = %e, "probe: rpc call failed");
            false
        }
    };

    // Record a sample for every known operator. In v1, the ALB-level
    // probe is the same signal for every operator on the registry;
    // v2 will probe each authority's own RPC endpoint independently.
    let now = chrono::Utc::now();
    let operator_ids = sqlx::query_scalar::<_, i64>("SELECT authority_id FROM operators")
        .fetch_all(pool)
        .await?;

    for aid in operator_ids {
        sqlx::query(
            "INSERT INTO uptime_samples (authority_id, sample_at, ok, latency_ms) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (authority_id, sample_at) DO NOTHING",
        )
        .bind(aid)
        .bind(now)
        .bind(ok)
        .bind(if ok { Some(latency_ms) } else { None })
        .execute(pool)
        .await?;
    }

    Ok(())
}
