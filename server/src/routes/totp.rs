use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use base32::Alphabet;
use ourshell_proto::sync::{TotpConfirm, TotpSetupResponse};
use rand::RngCore;
use totp_rs::{Algorithm, Totp};

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::sqlx;
use crate::state::AppState;

const SECRET_LEN: usize = 20;

fn build_totp(secret: &[u8], email: &str) -> AppResult<Totp> {
    totp_rs::Builder::new()
        .with_algorithm(Algorithm::SHA1)
        .with_secret(secret.to_vec())
        .with_issuer(Some("OurShell"))
        .with_account_name(email)
        .build()
        .map_err(|_| AppError::internal("failed to build TOTP"))
}

/// Verifies `code` against a raw secret. Used both by `/totp/confirm` and by
/// the login flow.
pub fn verify_code(secret: &[u8], code: &str) -> bool {
    match totp_rs::Builder::new()
        .with_algorithm(Algorithm::SHA1)
        .with_secret(secret.to_vec())
        .build()
    {
        Ok(totp) => totp.check_current(code).is_some(),
        Err(_) => false,
    }
}

fn aad(user_id: uuid::Uuid) -> Vec<u8> {
    format!("totp:{user_id}").into_bytes()
}

pub async fn setup(
    auth: AuthUser,
    State(state): State<AppState>,
) -> AppResult<Json<TotpSetupResponse>> {
    let (email,): (String,) = sqlx::query_as("SELECT email FROM users WHERE id = $1")
        .bind(auth.user_id)
        .fetch_one(state.pool())
        .await?;

    let mut secret = vec![0u8; SECRET_LEN];
    rand::rngs::OsRng.fill_bytes(&mut secret);

    let totp = build_totp(&secret, &email)?;
    let otpauth_url = totp
        .to_url()
        .map_err(|_| AppError::internal("failed to build otpauth url"))?;
    let secret_base32 = base32::encode(Alphabet::Rfc4648 { padding: false }, &secret);

    let encrypted = crate::crypto::encrypt(
        &state.config().server_master_key,
        &secret,
        &aad(auth.user_id),
    );
    sqlx::query(
        "INSERT INTO totp (user_id, org_id, secret, confirmed) VALUES ($1, $2, $3, false) \
         ON CONFLICT (user_id) DO UPDATE SET secret = EXCLUDED.secret, confirmed = false",
    )
    .bind(auth.user_id)
    .bind(auth.org_id)
    .bind(&encrypted)
    .execute(state.pool())
    .await?;

    Ok(Json(TotpSetupResponse {
        secret_base32,
        otpauth_url,
    }))
}

pub async fn confirm(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(body): Json<TotpConfirm>,
) -> AppResult<StatusCode> {
    let row: Option<(Vec<u8>,)> = sqlx::query_as("SELECT secret FROM totp WHERE user_id = $1")
        .bind(auth.user_id)
        .fetch_optional(state.pool())
        .await?;
    let Some((encrypted,)) = row else {
        return Err(AppError::bad_request(
            "totp_not_started",
            "call /totp/setup first",
        ));
    };
    let secret = crate::crypto::decrypt(
        &state.config().server_master_key,
        &encrypted,
        &aad(auth.user_id),
    )
    .map_err(|_| AppError::internal("failed to decrypt totp secret"))?;

    if !verify_code(&secret, &body.code) {
        return Err(AppError::bad_request(
            "invalid_code",
            "the TOTP code did not match",
        ));
    }

    sqlx::query("UPDATE totp SET confirmed = true WHERE user_id = $1")
        .bind(auth.user_id)
        .execute(state.pool())
        .await?;
    sqlx::query("UPDATE users SET totp_enabled = true WHERE id = $1")
        .bind(auth.user_id)
        .execute(state.pool())
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn disable(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(body): Json<TotpConfirm>,
) -> AppResult<StatusCode> {
    let row: Option<(Vec<u8>,)> =
        sqlx::query_as("SELECT secret FROM totp WHERE user_id = $1 AND confirmed = true")
            .bind(auth.user_id)
            .fetch_optional(state.pool())
            .await?;
    let Some((encrypted,)) = row else {
        return Err(AppError::bad_request(
            "totp_not_enabled",
            "TOTP is not enabled",
        ));
    };
    let secret = crate::crypto::decrypt(
        &state.config().server_master_key,
        &encrypted,
        &aad(auth.user_id),
    )
    .map_err(|_| AppError::internal("failed to decrypt totp secret"))?;
    if !verify_code(&secret, &body.code) {
        return Err(AppError::new(
            StatusCode::UNAUTHORIZED,
            "totp_invalid",
            "invalid TOTP code",
        ));
    }

    sqlx::query("DELETE FROM totp WHERE user_id = $1")
        .bind(auth.user_id)
        .execute(state.pool())
        .await?;
    sqlx::query("UPDATE users SET totp_enabled = false WHERE id = $1")
        .bind(auth.user_id)
        .execute(state.pool())
        .await?;

    Ok(StatusCode::NO_CONTENT)
}
