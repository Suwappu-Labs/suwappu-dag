//! Foundation-admin HTTP endpoints. Bearer-token authenticated.
//!
//! v1 surface:
//!
//! - `POST /admin/operators` — register a new operator (called
//!   after a governance admit lands; ties an `authority_id` to a
//!   display label).
//! - `POST /admin/award` — credit a bug-bounty or hackathon
//!   award per the POINTS.md severity matrix.
//! - `GET  /admin/operators` — list operators (audit).
//! - `GET  /admin/awards/:authority_id` — list awards (audit).

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use tracing::warn;

/// Shared state passed into each admin handler.
#[derive(Clone)]
pub struct AdminState {
    /// Postgres pool.
    pub pool: PgPool,
    /// Bearer token expected in the Authorization header. Foundation-
    /// supplied via env var at daemon startup.
    pub admin_token: String,
}

fn check_auth(headers: &HeaderMap, expected: &str) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if auth.strip_prefix("Bearer ") != Some(expected) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "missing or wrong bearer token" })),
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct OperatorIn {
    pub authority_id: i64,
    pub label: String,
    #[serde(default)]
    pub is_seed: bool,
}

#[derive(Debug, Serialize)]
pub struct OperatorOut {
    pub authority_id: i64,
    pub label: String,
    pub is_seed: bool,
    pub joined_at: chrono::DateTime<chrono::Utc>,
}

/// `POST /admin/operators` — register or update an operator row.
/// Idempotent on `authority_id`: re-registering with a new label
/// updates the label in place.
pub async fn handle_register_operator(
    State(state): State<AdminState>,
    headers: HeaderMap,
    Json(body): Json<OperatorIn>,
) -> impl IntoResponse {
    if let Err(resp) = check_auth(&headers, &state.admin_token) {
        return resp.into_response();
    }

    let result = sqlx::query(
        "INSERT INTO operators (authority_id, label, is_seed) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (authority_id) DO UPDATE \
         SET label = EXCLUDED.label, \
             is_seed = EXCLUDED.is_seed \
         RETURNING authority_id, label, is_seed, joined_at",
    )
    .bind(body.authority_id)
    .bind(&body.label)
    .bind(body.is_seed)
    .fetch_one(&state.pool)
    .await;

    match result {
        Ok(row) => {
            let out = OperatorOut {
                authority_id: row.get("authority_id"),
                label: row.get("label"),
                is_seed: row.get("is_seed"),
                joined_at: row.get("joined_at"),
            };
            (StatusCode::OK, Json(serde_json::to_value(out).unwrap())).into_response()
        }
        Err(e) => {
            warn!(error = %e, "admin: register_operator failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct AwardIn {
    pub authority_id: i64,
    /// "bug_bounty" or "hackathon"
    pub kind: String,
    pub points: i64,
    #[serde(default)]
    pub reason: Option<String>,
    /// Who awarded this — foundation staffer name or initials.
    /// Required for audit trail.
    pub awarded_by: String,
}

#[derive(Debug, Serialize)]
pub struct AwardOut {
    pub id: i64,
    pub authority_id: i64,
    pub kind: String,
    pub points: i64,
    pub reason: Option<String>,
    pub awarded_at: chrono::DateTime<chrono::Utc>,
    pub awarded_by: String,
}

/// `POST /admin/award` — credit a manual award (bug bounty or
/// hackathon). Per POINTS.md: bug_bounty ∈ {5000, 15000, 50000};
/// hackathon ∈ [1000, 10000]. Soft-validation only — foundation
/// has discretion within those bands and may override with a
/// `reason` justification.
pub async fn handle_award(
    State(state): State<AdminState>,
    headers: HeaderMap,
    Json(body): Json<AwardIn>,
) -> impl IntoResponse {
    if let Err(resp) = check_auth(&headers, &state.admin_token) {
        return resp.into_response();
    }
    if body.kind != "bug_bounty" && body.kind != "hackathon" {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "kind must be 'bug_bounty' or 'hackathon'"
            })),
        )
            .into_response();
    }
    if body.points <= 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "points must be positive" })),
        )
            .into_response();
    }
    if body.awarded_by.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "awarded_by required (audit trail)" })),
        )
            .into_response();
    }

    let result = sqlx::query(
        "INSERT INTO manual_awards (authority_id, kind, points, reason, awarded_by) \
         VALUES ($1, $2, $3, $4, $5) \
         RETURNING id, authority_id, kind, points, reason, awarded_at, awarded_by",
    )
    .bind(body.authority_id)
    .bind(&body.kind)
    .bind(body.points)
    .bind(body.reason.as_deref())
    .bind(&body.awarded_by)
    .fetch_one(&state.pool)
    .await;

    match result {
        Ok(row) => {
            let out = AwardOut {
                id: row.get("id"),
                authority_id: row.get("authority_id"),
                kind: row.get("kind"),
                points: row.get("points"),
                reason: row.try_get("reason").ok(),
                awarded_at: row.get("awarded_at"),
                awarded_by: row.get("awarded_by"),
            };
            (StatusCode::OK, Json(serde_json::to_value(out).unwrap())).into_response()
        }
        Err(e) => {
            warn!(error = %e, "admin: award failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

/// `GET /admin/operators` — list operators (audit trail).
pub async fn handle_list_operators(
    State(state): State<AdminState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(resp) = check_auth(&headers, &state.admin_token) {
        return resp.into_response();
    }
    let result = sqlx::query(
        "SELECT authority_id, label, is_seed, joined_at \
           FROM operators ORDER BY authority_id ASC",
    )
    .fetch_all(&state.pool)
    .await;
    match result {
        Ok(rows) => {
            let out: Vec<OperatorOut> = rows
                .into_iter()
                .map(|r| OperatorOut {
                    authority_id: r.get("authority_id"),
                    label: r.get("label"),
                    is_seed: r.get("is_seed"),
                    joined_at: r.get("joined_at"),
                })
                .collect();
            (StatusCode::OK, Json(out)).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// `GET /admin/awards/:authority_id` — list awards for one
/// operator (audit trail). Returns newest first.
pub async fn handle_list_awards(
    State(state): State<AdminState>,
    headers: HeaderMap,
    Path(authority_id): Path<i64>,
) -> impl IntoResponse {
    if let Err(resp) = check_auth(&headers, &state.admin_token) {
        return resp.into_response();
    }
    let result = sqlx::query(
        "SELECT id, authority_id, kind, points, reason, awarded_at, awarded_by \
           FROM manual_awards WHERE authority_id = $1 \
          ORDER BY awarded_at DESC",
    )
    .bind(authority_id)
    .fetch_all(&state.pool)
    .await;
    match result {
        Ok(rows) => {
            let out: Vec<AwardOut> = rows
                .into_iter()
                .map(|r| AwardOut {
                    id: r.get("id"),
                    authority_id: r.get("authority_id"),
                    kind: r.get("kind"),
                    points: r.get("points"),
                    reason: r.try_get("reason").ok(),
                    awarded_at: r.get("awarded_at"),
                    awarded_by: r.get("awarded_by"),
                })
                .collect();
            (StatusCode::OK, Json(out)).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}
