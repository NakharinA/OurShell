use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use ourshell_proto::sync::{AddVaultMemberRequest, CreateVaultRequest, IdResponse};

use crate::auth::AuthUser;
use crate::db;
use crate::error::{AppError, AppResult};
use crate::sqlx;
use crate::state::AppState;

pub async fn create(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(body): Json<CreateVaultRequest>,
) -> AppResult<Json<IdResponse>> {
    let vault_id = db::parse_uuid(&body.vault_id, "vault_id")?;
    if body.members.is_empty() {
        return Err(AppError::bad_request(
            "no_members",
            "a team vault needs at least one member",
        ));
    }

    let mut member_ids = Vec::with_capacity(body.members.len());
    for m in &body.members {
        let id = db::parse_uuid(&m.user_id, "members[].user_id")?;
        if !db::user_in_org(state.pool(), id, auth.org_id).await? {
            return Err(AppError::bad_request(
                "unknown_member",
                format!("{} is not a member of this org", m.user_id),
            ));
        }
        member_ids.push(id);
    }
    if !member_ids.contains(&auth.user_id) {
        return Err(AppError::bad_request(
            "creator_not_sealed",
            "include your own sealed copy of the vault key in members",
        ));
    }

    sqlx::query("INSERT INTO vaults (id, org_id, kind, name) VALUES ($1, $2, 'team', $3)")
        .bind(vault_id)
        .bind(auth.org_id)
        .bind(&body.name)
        .execute(state.pool())
        .await
        .map_err(|e| match e {
            sqlx::Error::Database(d) if d.is_unique_violation() => {
                AppError::conflict("a vault with this id already exists")
            }
            other => AppError::from(other),
        })?;

    for (member, member_id) in body.members.iter().zip(member_ids.iter()) {
        sqlx::query(
            "INSERT INTO vault_members (vault_id, org_id, user_id, key_id, sealed_vault_key) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(vault_id)
        .bind(auth.org_id)
        .bind(member_id)
        .bind(&body.key_id)
        .bind(&member.sealed_vault_key)
        .execute(state.pool())
        .await?;
    }

    db::log_event(
        state.pool(),
        auth.org_id,
        Some(auth.user_id),
        "vault_create",
        Some(&vault_id.to_string()),
    )
    .await?;

    Ok(Json(IdResponse {
        id: vault_id.to_string(),
    }))
}

pub async fn add_member(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(vault_id): Path<String>,
    Json(body): Json<AddVaultMemberRequest>,
) -> AppResult<StatusCode> {
    let vault_id = db::parse_uuid(&vault_id, "vault_id")?;
    if !db::is_vault_member(state.pool(), vault_id, auth.user_id).await? {
        return Err(AppError::forbidden(
            "not_a_member",
            "you must already be a member of this vault to share it",
        ));
    }
    let member_id = db::parse_uuid(&body.user_id, "user_id")?;
    if !db::user_in_org(state.pool(), member_id, auth.org_id).await? {
        return Err(AppError::bad_request(
            "unknown_member",
            "that user is not in this org",
        ));
    }

    sqlx::query(
        "INSERT INTO vault_members (vault_id, org_id, user_id, key_id, sealed_vault_key) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (vault_id, user_id) DO UPDATE \
         SET key_id = EXCLUDED.key_id, sealed_vault_key = EXCLUDED.sealed_vault_key",
    )
    .bind(vault_id)
    .bind(auth.org_id)
    .bind(member_id)
    .bind(&body.key_id)
    .bind(&body.sealed_vault_key)
    .execute(state.pool())
    .await?;

    db::log_event(
        state.pool(),
        auth.org_id,
        Some(auth.user_id),
        "vault_share",
        Some(&format!("{vault_id}:{member_id}")),
    )
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove_member(
    auth: AuthUser,
    State(state): State<AppState>,
    Path((vault_id, user_id)): Path<(String, String)>,
) -> AppResult<StatusCode> {
    let vault_id = db::parse_uuid(&vault_id, "vault_id")?;
    if !db::is_vault_member(state.pool(), vault_id, auth.user_id).await? {
        return Err(AppError::forbidden(
            "not_a_member",
            "you must be a member of this vault",
        ));
    }
    let target = db::parse_uuid(&user_id, "user_id")?;
    sqlx::query("DELETE FROM vault_members WHERE vault_id = $1 AND user_id = $2")
        .bind(vault_id)
        .bind(target)
        .execute(state.pool())
        .await?;

    db::log_event(
        state.pool(),
        auth.org_id,
        Some(auth.user_id),
        "vault_unshare",
        Some(&format!("{vault_id}:{target}")),
    )
    .await?;

    Ok(StatusCode::NO_CONTENT)
}
