//! Scoring loop — rolls up uptime + cert counts into per-epoch
//! `epoch_points` rows. Manual awards (bug bounty, hackathon) stay
//! in `manual_awards`; the leaderboard SUM/JOIN happens at read
//! time so awards are reflected without a re-rollup.
//!
//! v1 formula (mirrors `docs/testnet/POINTS.md`):
//!
//! - **Uptime tier**: probe success rate within the epoch window
//!   →  ≥ 99% → 100; ≥ 95% → 50; else 0.
//! - **Cert tier**: `floor(count / 1000)`, capped at 50 points per
//!   epoch. Reads from the `certs_observed` table; populated either
//!   by the foundation-admin endpoint `POST /admin/certs` or by
//!   the (future) S3 NDJSON ingest task that consumes operator-
//!   uploaded `events.ndjson` rotations. Operators with no
//!   `certs_observed` row for an epoch score 0 cert points
//!   (LEFT JOIN), so the rollup query is uniform regardless of
//!   whether ingest has populated the table.
//! - **Manual awards (bug bounty + hackathon)** stay out of the
//!   epoch rollup — they're summed at read time in
//!   `compute_leaderboard`.

use sqlx::{PgPool, Row};
use tracing::{info, warn};

use crate::SCORE_INTERVAL;

/// How long an "epoch" is for the points rollup. The actual
/// on-chain epoch is set by `rounds_per_epoch × round_ms` in
/// the genesis (testnet uses 4096 × 250ms = ~17 min). For
/// scoring we DO NOT need to track chain epochs precisely —
/// we just need a stable bucketing of probes. A 1-hour bucket
/// is operator-friendly (matches the hourly events.ndjson
/// rotation in VALIDATOR-OPERATORS.md) and gives the leaderboard
/// 24 buckets per day to plot.
pub const SCORE_BUCKET_SECS: i64 = 3600;

/// Probe sample count expected per epoch bucket if every probe
/// landed: `bucket_secs / probe_interval_secs`. Used as the
/// denominator for the uptime % computation.
pub const SAMPLES_PER_BUCKET: i64 = SCORE_BUCKET_SECS / 60;

/// Run the scoring loop forever. Exits only on a fatal
/// Postgres error.
pub async fn run_scoring_loop(pool: PgPool) {
    let mut tick = tokio::time::interval(SCORE_INTERVAL);
    loop {
        tick.tick().await;
        if let Err(e) = score_once(&pool).await {
            warn!(error = %e, "scoring: tick failed");
        }
    }
}

async fn score_once(pool: &PgPool) -> Result<(), crate::ProgramError> {
    // Compute the "current epoch bucket" as
    // floor(now_unix / SCORE_BUCKET_SECS). The scoring task
    // rolls up the previous bucket on each tick — by the time we
    // tick, that bucket is complete.
    let now_unix = chrono::Utc::now().timestamp();
    let current_bucket = now_unix / SCORE_BUCKET_SECS;
    let previous_bucket = current_bucket - 1;

    // Bucket bounds for the SQL window.
    let bucket_start =
        chrono::DateTime::<chrono::Utc>::from_timestamp(previous_bucket * SCORE_BUCKET_SECS, 0)
            .expect("epoch math");
    let bucket_end =
        chrono::DateTime::<chrono::Utc>::from_timestamp(current_bucket * SCORE_BUCKET_SECS, 0)
            .expect("epoch math");

    // For each known operator, compute the bucket's uptime samples
    // + the cert count for the bucket. LEFT JOIN against
    // certs_observed so operators with no cert rows for the bucket
    // still get a 0-count row.
    let rows = sqlx::query(
        "SELECT u.authority_id, \
                u.ok_samples, \
                u.total_samples, \
                COALESCE(c.count, 0) AS cert_count \
           FROM ( \
                SELECT authority_id, \
                       COUNT(*) FILTER (WHERE ok) AS ok_samples, \
                       COUNT(*)                    AS total_samples \
                  FROM uptime_samples \
                 WHERE sample_at >= $1 AND sample_at < $2 \
                 GROUP BY authority_id \
           ) u \
           LEFT JOIN certs_observed c \
                  ON c.authority_id = u.authority_id \
                 AND c.epoch        = $3",
    )
    .bind(bucket_start)
    .bind(bucket_end)
    .bind(previous_bucket)
    .fetch_all(pool)
    .await?;

    if rows.is_empty() {
        // No samples yet (probe loop hasn't started or there are
        // no operators registered yet). Nothing to roll up.
        return Ok(());
    }

    for row in rows {
        let aid: i64 = row.try_get("authority_id")?;
        let ok_samples: i64 = row.try_get("ok_samples")?;
        let total_samples: i64 = row.try_get("total_samples")?;
        let cert_count: i64 = row.try_get("cert_count")?;

        let uptime_points = compute_uptime_points(ok_samples, total_samples);
        let cert_points = compute_cert_points(cert_count);

        sqlx::query(
            "INSERT INTO epoch_points \
                 (authority_id, epoch, uptime_points, cert_points, \
                  bug_bounty_points, hackathon_points, computed_at) \
             VALUES ($1, $2, $3, $4, 0, 0, NOW()) \
             ON CONFLICT (authority_id, epoch) DO UPDATE \
             SET uptime_points = EXCLUDED.uptime_points, \
                 cert_points = EXCLUDED.cert_points, \
                 computed_at = NOW()",
        )
        .bind(aid)
        .bind(previous_bucket)
        .bind(uptime_points)
        .bind(cert_points)
        .execute(pool)
        .await?;
    }

    info!(epoch = previous_bucket, "scoring: rolled up bucket");
    Ok(())
}

/// Maximum cert points per epoch bucket. Caps the contribution
/// of a single epoch so a burst of cert observations can't
/// dominate the leaderboard.
pub const CERT_POINTS_CAP_PER_EPOCH: i64 = 50;

/// Per POINTS.md: 1 cert point per 1000 observed certs in the
/// epoch, capped at `CERT_POINTS_CAP_PER_EPOCH` (50/epoch).
/// `cert_count == 0` returns 0; negative inputs are clamped to
/// 0 (defensive; the schema disallows negatives but the JOIN
/// path produces an `i64`).
pub fn compute_cert_points(cert_count: i64) -> i64 {
    if cert_count <= 0 {
        return 0;
    }
    (cert_count / 1000).min(CERT_POINTS_CAP_PER_EPOCH)
}

/// Pure compute — extracted for unit testing.
///
/// Tier matches the public POINTS.md formula:
/// - uptime ratio ≥ 99% → 100 points
/// - uptime ratio ≥ 95% → 50 points
/// - else → 0
///
/// `total_samples == 0` returns 0 (no data → no credit).
pub fn compute_uptime_points(ok_samples: i64, total_samples: i64) -> i64 {
    if total_samples == 0 {
        return 0;
    }
    // Integer math — multiply first to keep the ratio precise.
    let ratio_pct = (ok_samples * 100) / total_samples;
    if ratio_pct >= 99 {
        100
    } else if ratio_pct >= 95 {
        50
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uptime_tier_99_percent_or_better() {
        // 60 of 60 = 100%
        assert_eq!(compute_uptime_points(60, 60), 100);
        // 59 of 60 = 98.3% → bucket 95-98 → 50
        assert_eq!(compute_uptime_points(59, 60), 50);
    }

    #[test]
    fn uptime_tier_95_to_99() {
        // 57 of 60 = 95% exactly → 50
        assert_eq!(compute_uptime_points(57, 60), 50);
        // 56 of 60 = 93.3% → 0
        assert_eq!(compute_uptime_points(56, 60), 0);
    }

    #[test]
    fn uptime_zero_samples_zero_points() {
        assert_eq!(compute_uptime_points(0, 0), 0);
    }

    #[test]
    fn uptime_all_failed() {
        assert_eq!(compute_uptime_points(0, 60), 0);
    }

    #[test]
    fn cert_points_zero_count_zero() {
        assert_eq!(compute_cert_points(0), 0);
    }

    #[test]
    fn cert_points_below_first_step_zero() {
        // 999 certs is still < 1 point.
        assert_eq!(compute_cert_points(999), 0);
    }

    #[test]
    fn cert_points_per_thousand() {
        assert_eq!(compute_cert_points(1_000), 1);
        assert_eq!(compute_cert_points(1_500), 1); // floor
        assert_eq!(compute_cert_points(2_000), 2);
        assert_eq!(compute_cert_points(25_000), 25);
    }

    #[test]
    fn cert_points_capped_at_50() {
        // Per POINTS.md the cap is 50/epoch regardless of count.
        assert_eq!(compute_cert_points(50_000), CERT_POINTS_CAP_PER_EPOCH);
        assert_eq!(compute_cert_points(50_999), CERT_POINTS_CAP_PER_EPOCH);
        assert_eq!(compute_cert_points(1_000_000), CERT_POINTS_CAP_PER_EPOCH);
        assert_eq!(
            compute_cert_points(i64::MAX),
            CERT_POINTS_CAP_PER_EPOCH,
            "huge counts must not overflow the cap"
        );
    }

    /// Defensive: negative counts (impossible per schema) clamp to 0
    /// rather than producing a negative-points oddity.
    #[test]
    fn cert_points_negative_clamps_to_zero() {
        assert_eq!(compute_cert_points(-1), 0);
        assert_eq!(compute_cert_points(i64::MIN), 0);
    }
}
