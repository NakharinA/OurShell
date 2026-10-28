use axum::extract::State;
use axum::Json;
use ourshell_proto::opaque::{opaque_ke, CipherSuite};
use ourshell_proto::sync::{
    AccountKeysUpload, RegisterFinish, RegisterStart, RegisterStartResponse, SessionResponse,
    VaultKeyInfo,
};
use uuid::Uuid;

use crate::db;
use crate::error::{AppError, AppResult};
use crate::sqlx;
use crate::state::AppState;

pub fn normalize_email(email: &str) -> String {
    email.trim().to_ascii_lowercase()
}

pub async fn start(
    State(state): State<AppState>,
    Json(body): Json<RegisterStart>,
) -> AppResult<Json<RegisterStartResponse>> {
    let email = normalize_email(&body.email);
    if email.is_empty() || !email.contains('@') {
        return Err(AppError::bad_request(
            "invalid_email",
            "email looks invalid",
        ));
    }

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
    sqlx::query("INSERT INTO pending_registrations (id, email, invite_code) VALUES ($1, $2, $3)")
        .bind(registration_id)
        .bind(&email)
        .bind(&body.invite_code)
        .execute(state.pool())
        .await?;

    Ok(Json(RegisterStartResponse {
        registration_id: registration_id.to_string(),
        registration_response: result.message.serialize().to_vec(),
    }))
}

pub async fn finish(
    State(state): State<AppState>,
    Json(body): Json<RegisterFinish>,
) -> AppResult<Json<SessionResponse>> {
    let registration_id = db::parse_uuid(&body.registration_id, "registration_id")?;
    let device_id = db::parse_uuid(&body.device.device_id, "device.device_id")?;
    let vault_id = db::parse_uuid(&body.vault.vault_id, "vault.vault_id")?;

    let pending: Option<(String, Option<String>)> = sqlx::query_as(
        "DELETE FROM pending_registrations WHERE id = $1 RETURNING email, invite_code",
    )
    .bind(registration_id)
    .fetch_optional(state.pool())
    .await?;
    let Some((email, invite_code)) = pending else {
        return Err(AppError::bad_request(
            "invalid_registration",
            "unknown or expired registration_id",
        ));
    };

    let upload =
        opaque_ke::RegistrationUpload::<CipherSuite>::deserialize(&body.registration_upload)
            .map_err(|_| {
                AppError::bad_request("invalid_request", "malformed registration_upload")
            })?;
    let password_file = opaque_ke::ServerRegistration::<CipherSuite>::finish(upload);

    let config = state.config();
    let is_bootstrap = db::total_user_count(state.pool()).await? == 0;
    let user_id = Uuid::new_v4();

    let mut consumed_invite_code: Option<String> = None;
    let (org_id, role): (Uuid, &str) = if is_bootstrap {
        let org_id = db::create_org(state.pool(), "Default Org").await?;
        (org_id, "owner")
    } else {
        if !config.signups_enabled {
            return Err(AppError::forbidden(
                "signups_disabled",
                "signups are currently disabled on this server",
            ));
        }
        let org_id = db::default_org_id(state.pool())
            .await?
            .ok_or_else(|| AppError::internal("no org exists but users do"))?;

        if config.invite_only {
            let code = invite_code.ok_or_else(|| {
                AppError::forbidden("invite_required", "an invite code is required to sign up")
            })?;
            let invite: Option<(String,)> = sqlx::query_as(
                "SELECT role FROM invites WHERE code = $1 AND org_id = $2 AND used_by IS NULL",
            )
            .bind(&code)
            .bind(org_id)
            .fetch_optional(state.pool())
            .await?;
            let Some((invite_role,)) = invite else {
                return Err(AppError::forbidden(
                    "invalid_invite",
                    "invite code is invalid or already used",
                ));
            };
            // Marked used only after the user row exists below (`used_by`
            // has a foreign key into `users`).
            consumed_invite_code = Some(code);
            (org_id, role_str(&invite_role))
        } else {
            (org_id, "member")
        }
    };

    sqlx::query("INSERT INTO users (id, org_id, email, role) VALUES ($1, $2, $3, $4)")
        .bind(user_id)
        .bind(org_id)
        .bind(&email)
        .bind(role)
        .execute(state.pool())
        .await
        .map_err(|e| match e {
            sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
                AppError::conflict("email is already registered")
            }
            other => AppError::from(other),
        })?;

    if let Some(code) = consumed_invite_code {
        sqlx::query("UPDATE invites SET used_by = $1, used_at = now() WHERE code = $2")
            .bind(user_id)
            .bind(&code)
            .execute(state.pool())
            .await?;
    }

    sqlx::query("INSERT INTO opaque_records (user_id, org_id, password_file) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(org_id)
        .bind(password_file.serialize().to_vec())
        .execute(state.pool())
        .await?;

    let recovery_account_wrap: Option<&[u8]> = (!body.account.recovery_account_wrap.is_empty())
        .then_some(body.account.recovery_account_wrap.as_slice());
    sqlx::query(
        "INSERT INTO account_keys (user_id, org_id, enc_public, sign_public, kdf, wrapped_account_keys, recovery_account_wrap) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(user_id)
    .bind(org_id)
    .bind(&body.account.enc_public)
    .bind(&body.account.sign_public)
    .bind(&body.account.kdf)
    .bind(&body.account.wrapped_account_keys)
    .bind(recovery_account_wrap)
    .execute(state.pool())
    .await?;

    sqlx::query("INSERT INTO vaults (id, org_id, kind, name) VALUES ($1, $2, 'personal', $3)")
        .bind(vault_id)
        .bind(org_id)
        .bind("Personal")
        .execute(state.pool())
        .await?;
    sqlx::query(
        "INSERT INTO vault_members (vault_id, org_id, user_id, key_id, sealed_vault_key, recovery_wrap) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(vault_id)
    .bind(org_id)
    .bind(user_id)
    .bind(&body.vault.key_id)
    .bind(&body.vault.sealed_vault_key)
    .bind(&body.vault.recovery_wrap)
    .execute(state.pool())
    .await?;

    db::upsert_device(
        state.pool(),
        org_id,
        user_id,
        device_id,
        &body.device.name,
        &body.device.public_key,
        config.max_devices_per_user,
    )
    .await?;

    let token = db::create_session(
        state.pool(),
        org_id,
        user_id,
        device_id,
        config.session_ttl_hours,
    )
    .await?;
    db::log_event(state.pool(), org_id, Some(user_id), "register", None).await?;

    Ok(Json(SessionResponse {
        session_token: token,
        user_id: user_id.to_string(),
        org_id: org_id.to_string(),
        account: AccountKeysUpload {
            enc_public: body.account.enc_public,
            sign_public: body.account.sign_public,
            kdf: body.account.kdf,
            wrapped_account_keys: body.account.wrapped_account_keys,
            recovery_account_wrap: body.account.recovery_account_wrap,
        },
        vaults: vec![VaultKeyInfo {
            vault_id: body.vault.vault_id,
            key_id: body.vault.key_id,
            name: "Personal".to_string(),
            kind: "personal".to_string(),
            sealed_vault_key: body.vault.sealed_vault_key,
            recovery_wrap: body.vault.recovery_wrap,
        }],
    }))
}

fn role_str(s: &str) -> &'static str {
    match s {
        "owner" => "owner",
        "admin" => "admin",
        _ => "member",
    }
}
