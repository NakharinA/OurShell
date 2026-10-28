//! `GET /v1/oidc/start` and `GET /v1/oidc/callback`. A successful round
//! trip only identifies the user by email; it never substitutes for the
//! OPAQUE login (see `ourshell_server::oidc`).

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::routes::register::normalize_email;
use crate::sqlx;
use crate::state::AppState;

pub async fn start(State(state): State<AppState>) -> AppResult<Redirect> {
    let Some(oidc) = state.oidc() else {
        return Err(AppError::not_implemented(
            "OIDC is not configured on this server (set OIDC_ISSUER, OIDC_CLIENT_ID, \
             OIDC_CLIENT_SECRET, OIDC_REDIRECT_URL)",
        ));
    };
    let (url, req_state) = oidc.authorize_url();
    sqlx::query("INSERT INTO oidc_requests (state, nonce, pkce_verifier) VALUES ($1, $2, $3)")
        .bind(&req_state.csrf_token)
        .bind(&req_state.nonce)
        .bind(&req_state.pkce_verifier)
        .execute(state.pool())
        .await?;
    Ok(Redirect::temporary(&url))
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: String,
    state: String,
}

pub async fn callback(
    State(state): State<AppState>,
    Query(q): Query<CallbackQuery>,
) -> AppResult<impl IntoResponse> {
    let Some(oidc) = state.oidc() else {
        return Err(AppError::not_implemented(
            "OIDC is not configured on this server",
        ));
    };

    let row: Option<(String, String)> =
        sqlx::query_as("DELETE FROM oidc_requests WHERE state = $1 RETURNING nonce, pkce_verifier")
            .bind(&q.state)
            .fetch_optional(state.pool())
            .await?;
    let Some((nonce, pkce_verifier)) = row else {
        return Err(AppError::bad_request(
            "invalid_state",
            "unknown or expired OIDC state",
        ));
    };

    let email = oidc
        .exchange(q.code, pkce_verifier, nonce)
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, "oidc_failed", e.to_string()))?;
    let email = normalize_email(&email);

    let exists: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM users WHERE email = $1")
        .bind(&email)
        .fetch_optional(state.pool())
        .await?;

    Ok(Json(json!({
        "email": email,
        "user_exists": exists.is_some(),
    })))
}
