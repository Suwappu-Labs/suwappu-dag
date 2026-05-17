//! `gsx-validator-program` — testnet points-accumulator daemon.
//!
//! Implements the public contract in [`docs/testnet/POINTS.md`].
//!
//! Workloads:
//!
//! - **Probe**: every 60s, hit `gsx_getEpoch` against the public
//!   RPC + each known seed/external validator. Record an
//!   uptime sample per authority in Postgres.
//! - **Score** (every 5 min): roll up the most recent epoch's
//!   uptime samples + cert counts + manual awards into
//!   `epoch_points`.
//! - **Serve**: HTTP API at `/leaderboard` (public read) +
//!   `/admin/award` (foundation auth-gated bug-bounty +
//!   hackathon entry).
//!
//! ## v1 scope limits
//!
//! - **Cert observation reads but does not auto-ingest**: the
//!   `certs_observed` table is now consumed by the scoring task
//!   (per `score::compute_cert_points`), and the foundation can
//!   populate it via `POST /admin/certs` (see `admin.rs`). The
//!   auto-ingest pipeline that consumes operator-uploaded
//!   `events.ndjson` files from S3 is a v2 workstream — gives
//!   the foundation a backfill path until then.
//! - **Single foundation instance**: the daemon is single-host.
//!   Decentralized scoring (multi-party MPC across seed
//!   validators) is a v2 consideration per POINTS.md.

#![forbid(unsafe_code)]
// Daemon crate — internal HTTP shapes (admin request/response
// structs in src/admin.rs) don't carry public-API obligations;
// doc-comments on every field would be noise. The public
// `docs/testnet/POINTS.md` doc is the authoritative contract.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use thiserror::Error;

pub mod admin;
pub mod leaderboard;
pub mod probe;
pub mod score;

/// Errors surfaced by the program crate. The HTTP layer in `main.rs`
/// maps each variant to a status code.
#[derive(Debug, Error)]
pub enum ProgramError {
    /// Postgres I/O or query failure.
    #[error("database: {0}")]
    Database(#[from] sqlx::Error),
    /// SDK / RPC failure during a probe.
    #[error("rpc: {0}")]
    Rpc(#[from] gsx_client::Error),
    /// Caller asked for an authority_id we don't know about.
    #[error("unknown authority_id: {0}")]
    UnknownAuthority(i64),
    /// Manual-award validation failure.
    #[error("invalid award: {0}")]
    InvalidAward(String),
}

/// A single operator registered with the points program.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operator {
    /// 0-indexed Authority Ring id.
    pub authority_id: i64,
    /// Human label (e.g. "us-east-1" for seeds, "acme-validator-co"
    /// for external operators).
    pub label: String,
    /// First-admit timestamp.
    pub joined_at: DateTime<Utc>,
    /// True if this is a foundation-operated seed validator.
    /// Seeds appear on the leaderboard but their points are
    /// EXCLUDED from the TGE conversion (the foundation's
    /// allocation is set separately).
    pub is_seed: bool,
}

/// Per-authority leaderboard entry, summed across all epochs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaderboardEntry {
    /// The operator's authority_id.
    pub authority_id: i64,
    /// Display label.
    pub label: String,
    /// True iff this is a foundation seed (not eligible for TGE
    /// conversion).
    pub is_seed: bool,
    /// Cumulative points across all earning categories.
    pub total_points: i64,
    /// Breakdown — sums the per-epoch components from `epoch_points`
    /// plus manual awards.
    pub uptime_points: i64,
    /// Cert-observation points. Populated by the scoring task
    /// from `certs_observed` per the POINTS.md formula
    /// (`floor(count/1000)`, capped at 50/epoch).
    pub cert_points: i64,
    /// Bug-bounty awards from `manual_awards`.
    pub bug_bounty_points: i64,
    /// Hackathon awards from `manual_awards`.
    pub hackathon_points: i64,
}

/// Compute the global leaderboard. Reads from `epoch_points`
/// (per-epoch rollups written by the scoring task) + `manual_awards`
/// (foundation-admin entries).
pub async fn compute_leaderboard(pool: &PgPool) -> Result<Vec<LeaderboardEntry>, ProgramError> {
    // Two-step query: aggregate the per-epoch rollup, then JOIN
    // operators for the labels + manual awards for the bug-bounty
    // / hackathon totals. Done as one statement so the
    // ranking is consistent within a snapshot. Uses the untyped
    // `sqlx::query` API so the build is hermetic — no live
    // Postgres needed at compile time.
    let sql = r#"
        WITH ep AS (
          SELECT authority_id,
                 SUM(uptime_points)    AS uptime_total,
                 SUM(cert_points)      AS cert_total
            FROM epoch_points
           GROUP BY authority_id
        ),
        mu AS (
          SELECT authority_id,
                 SUM(CASE WHEN kind = 'bug_bounty' THEN points ELSE 0 END) AS bug_total,
                 SUM(CASE WHEN kind = 'hackathon'  THEN points ELSE 0 END) AS hack_total
            FROM manual_awards
           GROUP BY authority_id
        )
        SELECT o.authority_id,
               o.label,
               o.is_seed,
               COALESCE(ep.uptime_total, 0) AS uptime_total,
               COALESCE(ep.cert_total,   0) AS cert_total,
               COALESCE(mu.bug_total,    0) AS bug_total,
               COALESCE(mu.hack_total,   0) AS hack_total
          FROM operators o
     LEFT JOIN ep USING (authority_id)
     LEFT JOIN mu USING (authority_id)
         ORDER BY (COALESCE(ep.uptime_total, 0)
                 + COALESCE(ep.cert_total,   0)
                 + COALESCE(mu.bug_total,    0)
                 + COALESCE(mu.hack_total,   0)) DESC,
                  o.authority_id ASC
        "#;
    let rows = sqlx::query(sql).fetch_all(pool).await?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let uptime: i64 = row.try_get("uptime_total")?;
        let cert: i64 = row.try_get("cert_total")?;
        let bug: i64 = row.try_get("bug_total")?;
        let hack: i64 = row.try_get("hack_total")?;
        out.push(LeaderboardEntry {
            authority_id: row.try_get("authority_id")?,
            label: row.try_get("label")?,
            is_seed: row.try_get("is_seed")?,
            total_points: uptime + cert + bug + hack,
            uptime_points: uptime,
            cert_points: cert,
            bug_bounty_points: bug,
            hackathon_points: hack,
        });
    }
    Ok(out)
}

/// Probe + scoring loop cadence.
pub const PROBE_INTERVAL: Duration = Duration::from_secs(60);
/// Score-rollup cadence. Mirrors the public POINTS.md commitment
/// of weekly leaderboard publication, but we recompute every
/// 5 minutes so the leaderboard reflects fresh probes.
pub const SCORE_INTERVAL: Duration = Duration::from_secs(300);

/// Initialize the database — runs the migrations bundled with this
/// crate. Idempotent.
pub async fn init_db(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}
