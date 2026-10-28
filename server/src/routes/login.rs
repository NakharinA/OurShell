use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use ourshell_proto::opaque::{opaque_ke, CipherSuite};
use ourshell_proto::sync::{
    AccountKeysUpload, LoginFinish, LoginStart, LoginStartResponse, SessionResponse, VaultKeyInfo,
};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::db;
use crate::error::{AppError, AppResult};
use crate::routes::register::normalize_email;
use crate::sqlx;
use crate::state::AppState;

pub async fn start(
    State(state): State<AppState>,
    Json(body): Json<LoginStart>,
) -> AppResult<Json<LoginStartResponse>> {
    let email = normalize_email(&body.email);
    let credential_request =
        opaque_ke::CredentialRequest::<CipherSuite>::deserialize(&body.credential_request)
            .map_err(|_| {
                AppError::bad_request("invalid_request", "malformed credential_request")
            })?;

    let user_row: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM users WHERE email = $1")
        .bind(&email)
        .fetch_optional(state.pool())
        .await?;

    let password_file = match user_row {
        Some((user_id,)) => {
            let row: Option<(Vec<u8>,)> =
                sqlx::query_as("SELECT password_file FROM opaque_records WHERE user_id = $1")
                    .bind(user_id)
                    .fetch_optional(state.pool())
                    .await?;
            row.and_then(|(bytes,)| {
                opaque_ke::ServerRegistration::<CipherSuite>::deserialize(&bytes).ok()
            })
        }
        None => None,
    };

    let server_login_start = opaque_ke::ServerLogin::start(
        &mut rand::rngs::OsRng,
        state.server_setup(),
        password_file,
        credential_request,
        email.as_bytes(),
        opaque_ke::ServerLoginParameters::default(),
    )
    .map_err(|_| AppError::internal("OPAQUE login start failed"))?;

    let login_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pending_logins (id, user_id, email, server_login_state) VALUES ($1, $2, $3, $4)",
    )
    .bind(login_id)
    .bind(user_row.map(|(id,)| id))
    .bind(&email)
    .bind(server_login_start.state.serialize().to_vec())
    .execute(state.pool())
    .await?;

    Ok(Json(LoginStartResponse {
        login_id: login_id.to_string(),
        credential_response: server_login_start.message.serialize().to_vec(),
    }))
}

pub async fn finish(
    State(state): State<AppState>,
    Json(body): Json<LoginFinish>,
) -> AppResult<Json<SessionResponse>> {
    let login_id = db::parse_uuid(&body.login_id, "login_id")?;
    let device_id = db::parse_uuid(&body.device.device_id, "device.device_id")?;

    let pending: Option<(Option<Uuid>, Vec<u8>)> = sqlx::query_as(
        "DELETE FROM pending_logins WHERE id = $1 RETURNING user_id, server_login_state",
    )
    .bind(login_id)
    .fetch_optional(state.pool())
    .await?;
    let Some((maybe_user_id, server_login_state)) = pending else {
        return Err(AppError::bad_request(
            "invalid_login",
            "unknown or expired login_id",
        ));
    };
    let Some(user_id) = maybe_user_id else {
        // No such account: still a well-formed failure, never a protocol crash.
        return Err(AppError::unauthorized("invalid email or password"));
    };

    let server_login = opaque_ke::ServerLogin::<CipherSuite>::deserialize(&server_login_state)
        .map_err(|_| AppError::internal("corrupt pending login state"))?;
    let finalization = opaque_ke::CredentialFinalization::<CipherSuite>::deserialize(
        &body.credential_finalization,
    )
    .map_err(|_| AppError::bad_request("invalid_request", "malformed credential_finalization"))?;
    server_login
        .finish(finalization, opaque_ke::ServerLoginParameters::default())
        .map_err(|_| AppError::unauthorized("invalid email or password"))?;

    let user: (Uuid, String, bool) =
        sqlx::query_as("SELECT org_id, role, disabled FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_one(state.pool())
            .await?;
    let (org_id, role, disabled) = user;
    if disabled {
        return Err(AppError::forbidden("user_disabled", "account is disabled"));
    }

    let totp_row: Option<(Vec<u8>,)> =
        sqlx::query_as("SELECT secret FROM totp WHERE user_id = $1 AND confirmed = true")
            .bind(user_id)
            .fetch_optional(state.pool())
            .await?;
    if let Some((encrypted_secret,)) = totp_row {
        let code = body.totp.as_deref().ok_or_else(|| {
            AppError::new(
                StatusCode::UNAUTHORIZED,
                "totp_required",
                "a TOTP code is required",
            )
        })?;
        let secret = crate::crypto::decrypt(
            &state.config().server_master_key,
            &encrypted_secret,
            format!("totp:{user_id}").as_bytes(),
        )
        .map_err(|_| AppError::internal("failed to decrypt totp secret"))?;
        if !crate::routes::totp::verify_code(&secret, code) {
            return Err(AppError::new(
                StatusCode::UNAUTHORIZED,
                "totp_invalid",
                "invalid TOTP code",
            ));
        }
    }

    db::upsert_device(
        state.pool(),
        org_id,
        user_id,
        device_id,
        &body.device.name,
        &body.device.public_key,
        state.config().max_devices_per_user,
    )
    .await?;

    let token = db::create_session(
        state.pool(),
        org_id,
        user_id,
        device_id,
        state.config().session_ttl_hours,
    )
    .await?;
    db::log_event(state.pool(), org_id, Some(user_id), "login", None).await?;

    let account: (String, String, String, Vec<u8>, Option<Vec<u8>>) = sqlx::query_as(
        "SELECT enc_public, sign_public, kdf, wrapped_account_keys, recovery_account_wrap FROM account_keys WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_one(state.pool())
    .await?;

    let vault_rows = db::user_vaults(state.pool(), user_id).await?;

    let vaults = vault_rows
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

    let _ = role; // included via AuthUser on later requests; not needed in the body today.

    Ok(Json(SessionResponse {
        session_token: token,
        user_id: user_id.to_string(),
        org_id: org_id.to_string(),
        account: AccountKeysUpload {
            enc_public: account.0,
            sign_public: account.1,
            kdf: account.2,
            wrapped_account_keys: account.3,
            recovery_account_wrap: account.4.unwrap_or_default(),
        },
        vaults,
    }))
}

pub async fn logout(auth: AuthUser, State(state): State<AppState>) -> AppResult<StatusCode> {
    sqlx::query("DELETE FROM sessions WHERE id = $1")
        .bind(auth.session_id)
        .execute(state.pool())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
