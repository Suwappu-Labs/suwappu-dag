//! HTTP read API for the leaderboard.
//!
//! Public — no auth. Returns the full leaderboard sorted by total
//! points descending. Foundation seeds are flagged but kept on
//! the same list (frontend can filter).
//!
//! Public routes carry `Access-Control-Allow-Origin: *` (see
//! [`add_public_cors`]) so browser frontends — the compute-provider
//! portal's earnings lookup in particular — can read them
//! cross-origin. Admin routes deliberately do not.

use axum::{extract::State, response::IntoResponse, response::Response, Json};
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

/// `map_response` middleware for the PUBLIC read routes only:
/// stamps `Access-Control-Allow-Origin: *` so browser frontends can
/// read the leaderboard cross-origin. The endpoints are unauthenticated
/// GETs returning public data, so the wildcard grants nothing a curl
/// couldn't already get. Simple GETs trigger no preflight, so no
/// OPTIONS handling is needed. Never layer this over the admin router.
pub async fn add_public_cors(mut response: Response) -> Response {
    response.headers_mut().insert(
        axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
        axum::http::HeaderValue::from_static("*"),
    );
    response
}

#[cfg(test)]
mod tests {
    use axum::{middleware::map_response, routing::get, Router};
    use tower::util::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn public_routes_carry_cors_header() {
        let app = Router::new()
            .route("/leaderboard", get(|| async { "[]" }))
            .layer(map_response(add_public_cors));
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/leaderboard")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .map(|v| v.to_str().unwrap()),
            Some("*"),
        );
    }

    #[tokio::test]
    async fn unlayered_routes_do_not_carry_cors_header() {
        let app = Router::new().route("/admin/thing", get(|| async { "no" }));
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/admin/thing")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response
            .headers()
            .get(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none());
    }
}
