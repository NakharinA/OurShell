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
use crate::state::AppState;

fn role_to_db(role: &OrgRole) -> &'static str {
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

pub async fn create_invite(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(body): Json<CreateInviteRequest>,
) -> AppResult<Json<CreateInviteResponse>> {
    auth.require_admin()?;
    let role = body.role.as_ref().map(role_to_db).unwrap_or("member");
    let code = generate_invite_code();
    sqlx::query("INSERT INTO invites (code, org_id, role, created_by) VALUES ($1, $2, $3, $4)")
        .bind(&code)
        .bind(auth.org_id)
        .bind(role)
        .bind(auth.user_id)
        .execute(state.pool())
        .await?;
    Ok(Json(CreateInviteResponse { code }))
}

pub async fn list_users(
    auth: AuthUser,
    State(state): State<AppState>,
) -> AppResult<Json<OrgUsersResponse>> {
    auth.require_admin()?;
    let rows: Vec<(Uuid, String, String, bool)> = sqlx::query_as(
        "SELECT id, email, role, disabled FROM users WHERE org_id = $1 ORDER BY created_at",
    )
    .bind(auth.org_id)
    .fetch_all(state.pool())
    .await?;
    let users = rows
        .into_iter()
        .map(|(id, email, role, disabled)| OrgUserInfo {
            user_id: id.to_string(),
            email,
            role: role_from_db(&role),
            disabled,
        })
        .collect();
    Ok(Json(OrgUsersResponse { users }))
}

pub async fn disable_user(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(user_id): Path<String>,
) -> AppResult<StatusCode> {
    auth.require_admin()?;
    let user_id = db::parse_uuid(&user_id, "user_id")?;
    let updated = sqlx::query("UPDATE users SET disabled = true WHERE id = $1 AND org_id = $2")
        .bind(user_id)
        .bind(auth.org_id)
        .execute(state.pool())
        .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::not_found(
            "user_not_found",
            "no such user in this org",
        ));
    }
    sqlx::query("DELETE FROM sessions WHERE user_id = $1")
        .bind(user_id)
        .execute(state.pool())
        .await?;
    db::log_event(
        state.pool(),
        auth.org_id,
        Some(auth.user_id),
        "user_disable",
        Some(&user_id.to_string()),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
