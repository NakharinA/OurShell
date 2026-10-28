use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use ourshell_proto::sync::{DeviceInfo, DeviceListResponse};

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppResult;
use crate::sqlx;
use crate::state::AppState;

pub async fn list(
    auth: AuthUser,
    State(state): State<AppState>,
) -> AppResult<Json<DeviceListResponse>> {
    let rows: Vec<(String, String, String, chrono::DateTime<chrono::Utc>, bool)> = sqlx::query_as(
        "SELECT id::text, name, public_key, created_at, revoked FROM devices WHERE user_id = $1 ORDER BY created_at",
    )
    .bind(auth.user_id)
    .fetch_all(state.pool())
    .await?;

    let devices = rows
        .into_iter()
        .map(
            |(device_id, name, public_key, created_at, revoked)| DeviceInfo {
                device_id,
                name,
                public_key,
                created_at: created_at.to_rfc3339(),
                revoked,
            },
        )
        .collect();

    Ok(Json(DeviceListResponse { devices }))
}

pub async fn revoke(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(device_id): Path<String>,
) -> AppResult<StatusCode> {
    let device_id = db::parse_uuid(&device_id, "device_id")?;

    sqlx::query("UPDATE devices SET revoked = true WHERE id = $1 AND user_id = $2")
        .bind(device_id)
        .bind(auth.user_id)
        .execute(state.pool())
        .await?;
    sqlx::query("DELETE FROM sessions WHERE device_id = $1")
        .bind(device_id)
        .execute(state.pool())
        .await?;
    db::log_event(
        state.pool(),
        auth.org_id,
        Some(auth.user_id),
        "device_revoke",
        Some(&device_id.to_string()),
    )
    .await?;

    Ok(StatusCode::NO_CONTENT)
}
