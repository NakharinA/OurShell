//! `POST /v1/account/password/start` + `/finish`: a fresh OPAQUE
//! registration for the *currently signed-in* user, replacing their
//! password file and account key material atomically. Used both for an
//! ordinary password change and for `vault recover` while still signed in
//! (see `docs/self-hosting.md`).

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use ourshell_proto::opaque::{opaque_ke, CipherSuite};
use ourshell_proto::sync::{
    AccountPasswordFinish, AccountPasswordStart, AccountPasswordStartResponse,
    AccountVaultsResponse, VaultKeyInfo,
};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::db;
use crate::error::{AppError, AppResult};
use crate::sqlx;
use crate::state::AppState;

/// This user's up-to-date vault list -- lets an already-signed-in session
/// pick up a team vault it was just added to (or notice a removal)
/// without a fresh login. See `ourshell_core::sync::sync`.
pub async fn vaults(
    auth: AuthUser,
    State(state): State<AppState>,
) -> AppResult<Json<AccountVaultsResponse>> {
    let rows = db::user_vaults(state.pool(), auth.user_id).await?;
    let vaults = rows
        .into_iter()
        .map(
            |(vault_id, kind, name, key_id, sealed_vault_key, recovery_wrap)| VaultKeyInfo {
                vault_id,
                key_id,
                name,
                kind,
                sealed_vault_key,
                recovery_wrap: recovery_wrap.unwrap_or_default(),
            },
        )
        .collect();
    Ok(Json(AccountVaultsResponse { vaults }))
}

pub async fn start(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(body): Json<AccountPasswordStart>,
) -> AppResult<Json<AccountPasswordStartResponse>> {
    let (email,): (String,) = sqlx::query_as("SELECT email FROM users WHERE id = $1")
        .bind(auth.user_id)
        .fetch_one(state.pool())
        .await?;

    let request =
        opaque_ke::RegistrationRequest::<CipherSuite>::deserialize(&body.registration_request)
            .map_err(|_| {
                AppError::bad_request("invalid_request", "malformed registration_request")
            })?;
    let result = opaque_ke::ServerRegistration::<CipherSuite>::start(
        state.server_setup(),
        request,
        email.as_bytes(),
    )
    .map_err(|_| AppError::internal("OPAQUE registration start failed"))?;

    let registration_id = Uuid::new_v4();
    sqlx::query("INSERT INTO pending_password_changes (id, user_id) VALUES ($1, $2)")
        .bind(registration_id)
        .bind(auth.user_id)
        .execute(state.pool())
        .await?;

    Ok(Json(AccountPasswordStartResponse {
        registration_id: registration_id.to_string(),
        registration_response: result.message.serialize().to_vec(),
    }))
}

pub async fn finish(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(body): Json<AccountPasswordFinish>,
) -> AppResult<StatusCode> {
    let registration_id = db::parse_uuid(&body.registration_id, "registration_id")?;
    let pending: Option<(Uuid,)> = sqlx::query_as(
        "DELETE FROM pending_password_changes WHERE id = $1 AND user_id = $2 RETURNING user_id",
    )
    .bind(registration_id)
    .bind(auth.user_id)
    .fetch_optional(state.pool())
    .await?;
    if pending.is_none() {
        return Err(AppError::bad_request(
            "invalid_registration",
            "unknown or expired registration_id",
        ));
    }

    let upload =
        opaque_ke::RegistrationUpload::<CipherSuite>::deserialize(&body.registration_upload)
            .map_err(|_| {
                AppError::bad_request("invalid_request", "malformed registration_upload")
            })?;
    let password_file = opaque_ke::ServerRegistration::<CipherSuite>::finish(upload);

    let recovery_account_wrap: Option<&[u8]> =
        (!body.recovery_account_wrap.is_empty()).then_some(body.recovery_account_wrap.as_slice());

    sqlx::query(
        "UPDATE opaque_records SET password_file = $1, updated_at = now() WHERE user_id = $2",
    )
    .bind(password_file.serialize().to_vec())
    .bind(auth.user_id)
    .execute(state.pool())
    .await?;
    sqlx::query(
        "UPDATE account_keys SET enc_public = $1, sign_public = $2, kdf = $3, wrapped_account_keys = $4, \
         recovery_account_wrap = $5, updated_at = now() WHERE user_id = $6",
    )
    .bind(&body.enc_public)
    .bind(&body.sign_public)
    .bind(&body.kdf)
    .bind(&body.wrapped_account_keys)
    .bind(recovery_account_wrap)
    .bind(auth.user_id)
    .execute(state.pool())
    .await?;
    // Revoke every other session for this user; keep the one making this
    // request so the client doesn't have to immediately re-authenticate.
    sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND id != $2")
        .bind(auth.user_id)
        .bind(auth.session_id)
        .execute(state.pool())
        .await?;

    db::log_event(
        state.pool(),
        auth.org_id,
        Some(auth.user_id),
        "password_change",
        None,
    )
    .await?;

    Ok(StatusCode::NO_CONTENT)
}
