//! HTTP read API for the leaderboard.
//!
//! Public — no auth. Returns the full leaderboard sorted by total
//! points descending. Foundation seeds are flagged but kept on
//! the same list (frontend can filter).

use axum::{extract::State, response::IntoResponse, Json};
use sqlx::PgPool;
use tracing::warn;

use crate::{compute_leaderboard, LeaderboardEntry};

/// Snapshot returned by `GET /leaderboard`. Includes the generation
/// timestamp so clients can detect stale caches.
#[derive(Debug, serde::Serialize)]
pub struct LeaderboardSnapshot {
    /// Server time at which the snapshot was computed.
    pub computed_at: chrono::DateTime<chrono::Utc>,
    /// Ordered list — highest-points operators first.
    pub entries: Vec<LeaderboardEntry>,
}

/// Handler for `GET /leaderboard`. Public; no auth.
pub async fn handle_leaderboard(State(pool): State<PgPool>) -> impl IntoResponse {
    match compute_leaderboard(&pool).await {
        Ok(entries) => {
            let snap = LeaderboardSnapshot {
                computed_at: chrono::Utc::now(),
                entries,
            };
            (axum::http::StatusCode::OK, Json(snap)).into_response()
        }
        Err(e) => {
            warn!(error = %e, "leaderboard: compute failed");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}
