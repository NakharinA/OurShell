use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;

use crate::sqlx;
use crate::state::AppState;
use crate::web::VERSION;

/// `mode` is `"ready"` here; the setup wizard's router answers `"setup"`.
/// Fails (503) when the database is unreachable, so Docker marks the
/// container unhealthy.
pub async fn health(State(state): State<AppState>) -> impl IntoResponse {
    match sqlx::query("SELECT 1").execute(state.pool()).await {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({ "status": "ok", "mode": "ready", "version": VERSION })),
        ),
        Err(e) => {
            tracing::warn!(error = %e, "health check: database unreachable");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(
                    json!({ "status": "degraded", "mode": "ready", "version": VERSION, "error": "database unreachable" }),
                ),
            )
        }
    }
}
