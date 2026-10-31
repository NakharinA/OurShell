use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use base32::Alphabet;
use ourshell_proto::sync::{
    CreateInviteRequest, CreateInviteResponse, OrgRole, OrgUserInfo, OrgUsersResponse,
};
use rand::RngCore;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::db;
use crate::error::{AppError, AppResult};
use crate::sqlx;
use crate::sqlx::PgPool;
use crate::state::AppState;

pub fn role_to_db(role: &OrgRole) -> &'static str {
    match role {
        OrgRole::Owner => "owner",
        OrgRole::Admin => "admin",
        OrgRole::Member => "member",
    }
}

fn role_from_db(role: &str) -> OrgRole {
    match role {
        "owner" => OrgRole::Owner,
        "admin" => OrgRole::Admin,
        _ => OrgRole::Member,
    }
}

fn generate_invite_code() -> String {
    let mut bytes = [0u8; 10];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    base32::encode(Alphabet::Rfc4648 { padding: false }, &bytes)
}

/// Creates an invite code for `org_id`. `created_by` is `None` when the
/// admin web page (which has no user account) creates it.
pub async fn create_invite_in(
    pool: &PgPool,
    org_id: Uuid,
    role: &str,
    created_by: Option<Uuid>,
) -> sqlx::Result<String> {
    let code = generate_invite_code();
    sqlx::query("INSERT INTO invites (code, org_id, role, created_by) VALUES ($1, $2, $3, $4)")
        .bind(&code)
        .bind(org_id)
        .bind(role)
        .bind(created_by)
        .execute(pool)
        .await?;
    Ok(code)
}

pub async fn org_users(pool: &PgPool, org_id: Uuid) -> sqlx::Result<Vec<OrgUserInfo>> {
    let rows: Vec<(Uuid, String, String, bool)> = sqlx::query_as(
        "SELECT id, email, role, disabled FROM users WHERE org_id = $1 ORDER BY created_at",
    )
    .bind(org_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, email, role, disabled)| OrgUserInfo {
            user_id: id.to_string(),
            email,
            role: role_from_db(&role),
            disabled,
        })
        .collect())
}

/// Disables (signing out every session) or re-enables a user in `org_id`.
pub async fn set_user_disabled(
    pool: &PgPool,
    org_id: Uuid,
    user_id: Uuid,
    disabled: bool,
    actor: Option<Uuid>,
) -> AppResult<()> {
    let updated = sqlx::query("UPDATE users SET disabled = $1 WHERE id = $2 AND org_id = $3")
        .bind(disabled)
        .bind(user_id)
        .bind(org_id)
        .execute(pool)
        .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::not_found(
            "user_not_found",
            "no such user in this org",
        ));
    }
    if disabled {
        sqlx::query("DELETE FROM sessions WHERE user_id = $1")
            .bind(user_id)
            .execute(pool)
            .await?;
    }
    db::log_event(
        pool,
        org_id,
        actor,
        if disabled {
            "user_disable"
        } else {
            "user_enable"
        },
        Some(&user_id.to_string()),
    )
    .await?;
    Ok(())
}

pub async fn create_invite(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(body): Json<CreateInviteRequest>,
) -> AppResult<Json<CreateInviteResponse>> {
    auth.require_admin()?;
    let role = body.role.as_ref().map(role_to_db).unwrap_or("member");
    let code = create_invite_in(state.pool(), auth.org_id, role, Some(auth.user_id)).await?;
    Ok(Json(CreateInviteResponse { code }))
}

pub async fn list_users(
    auth: AuthUser,
    State(state): State<AppState>,
) -> AppResult<Json<OrgUsersResponse>> {
    auth.require_admin()?;
    let users = org_users(state.pool(), auth.org_id).await?;
    Ok(Json(OrgUsersResponse { users }))
}

pub async fn disable_user(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(user_id): Path<String>,
) -> AppResult<StatusCode> {
    auth.require_admin()?;
    let user_id = db::parse_uuid(&user_id, "user_id")?;
    set_user_disabled(state.pool(), auth.org_id, user_id, true, Some(auth.user_id)).await?;
    Ok(StatusCode::NO_CONTENT)
}
