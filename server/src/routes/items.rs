use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::Utc;
use ourshell_proto::sync::{PullResponse, PushRequest, PushResponse, PushResult, SyncItem};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::db;
use crate::error::{AppError, AppResult};
use crate::sqlx;
use crate::state::AppState;

/// The server never inspects envelope content, but it does cap size.
const MAX_ENVELOPE_BYTES: usize = 256 * 1024;
const PULL_PAGE_SIZE: i64 = 500;

#[derive(Deserialize)]
pub struct PullQuery {
    #[serde(default)]
    since: i64,
}

fn item_aad(vault_id: Uuid, id: Uuid) -> Vec<u8> {
    format!("item:{vault_id}:{id}").into_bytes()
}

async fn require_membership(state: &AppState, vault_id: Uuid, user_id: Uuid) -> AppResult<()> {
    if db::is_vault_member(state.pool(), vault_id, user_id).await? {
        Ok(())
    } else {
        Err(AppError::forbidden(
            "not_a_member",
            "you are not a member of this vault",
        ))
    }
}

pub async fn pull(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(vault_id): Path<String>,
    Query(query): Query<PullQuery>,
) -> AppResult<Json<PullResponse>> {
    let vault_id = db::parse_uuid(&vault_id, "vault_id")?;
    require_membership(&state, vault_id, auth.user_id).await?;

    type ItemRow = (Uuid, String, i64, bool, Vec<u8>, chrono::DateTime<Utc>, i64);
    let rows: Vec<ItemRow> = sqlx::query_as(
        "SELECT id, kind, version, deleted, envelope, updated_at, seq \
         FROM items WHERE vault_id = $1 AND seq > $2 ORDER BY seq LIMIT $3",
    )
    .bind(vault_id)
    .bind(query.since)
    .bind(PULL_PAGE_SIZE)
    .fetch_all(state.pool())
    .await?;

    let mut cursor = query.since;
    let mut items = Vec::with_capacity(rows.len());
    for (id, kind, version, deleted, envelope, updated_at, seq) in rows {
        let plaintext = crate::crypto::decrypt(
            &state.config().server_master_key,
            &envelope,
            &item_aad(vault_id, id),
        )
        .map_err(|_| AppError::internal("failed to decrypt stored item"))?;
        items.push(SyncItem {
            id: id.to_string(),
            kind,
            version: version as u64,
            vault_id: vault_id.to_string(),
            deleted,
            envelope: String::from_utf8(plaintext)
                .map_err(|_| AppError::internal("stored item envelope is not valid utf-8"))?,
            updated_at: Some(updated_at.to_rfc3339()),
        });
        cursor = cursor.max(seq);
    }

    Ok(Json(PullResponse { items, cursor }))
}

pub async fn push(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(vault_id): Path<String>,
    Json(body): Json<PushRequest>,
) -> AppResult<Json<PushResponse>> {
    let vault_id = db::parse_uuid(&vault_id, "vault_id")?;
    require_membership(&state, vault_id, auth.user_id).await?;

    let mut results = Vec::with_capacity(body.items.len());
    for item in body.items {
        if item.vault_id != vault_id.to_string() {
            return Err(AppError::bad_request(
                "vault_mismatch",
                "item.vault_id does not match the URL's vault id",
            ));
        }
        if item.envelope.len() > MAX_ENVELOPE_BYTES {
            return Err(AppError::bad_request(
                "item_too_large",
                "envelope exceeds the size limit",
            ));
        }
        let id = db::parse_uuid(&item.id, "item.id")?;

        type CurrentItemRow = (i64, String, bool, Vec<u8>, chrono::DateTime<Utc>);
        let current: Option<CurrentItemRow> = sqlx::query_as(
            "SELECT version, kind, deleted, envelope, updated_at FROM items WHERE vault_id = $1 AND id = $2",
        )
        .bind(vault_id)
        .bind(id)
        .fetch_optional(state.pool())
        .await?;

        let encrypted = crate::crypto::encrypt(
            &state.config().server_master_key,
            item.envelope.as_bytes(),
            &item_aad(vault_id, id),
        );

        match current {
            None => {
                sqlx::query(
                    "INSERT INTO items (vault_id, org_id, id, kind, version, deleted, envelope, updated_at) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, now())",
                )
                .bind(vault_id)
                .bind(auth.org_id)
                .bind(id)
                .bind(&item.kind)
                .bind(item.version as i64)
                .bind(item.deleted)
                .bind(&encrypted)
                .execute(state.pool())
                .await?;
                results.push(PushResult {
                    id: item.id,
                    status: "ok".to_string(),
                    current: None,
                });
            }
            Some((
                current_version,
                current_kind,
                current_deleted,
                current_envelope,
                current_updated_at,
            )) => {
                if item.version as i64 == current_version + 1 {
                    sqlx::query(
                        "UPDATE items SET kind = $1, version = $2, deleted = $3, envelope = $4, \
                         updated_at = now(), seq = nextval('items_seq_seq') WHERE vault_id = $5 AND id = $6",
                    )
                    .bind(&item.kind)
                    .bind(item.version as i64)
                    .bind(item.deleted)
                    .bind(&encrypted)
                    .bind(vault_id)
                    .bind(id)
                    .execute(state.pool())
                    .await?;
                    results.push(PushResult {
                        id: item.id,
                        status: "ok".to_string(),
                        current: None,
                    });
                } else {
                    let plaintext = crate::crypto::decrypt(
                        &state.config().server_master_key,
                        &current_envelope,
                        &item_aad(vault_id, id),
                    )
                    .map_err(|_| AppError::internal("failed to decrypt stored item"))?;
                    results.push(PushResult {
                        id: item.id.clone(),
                        status: "conflict".to_string(),
                        current: Some(SyncItem {
                            id: item.id,
                            kind: current_kind,
                            version: current_version as u64,
                            vault_id: vault_id.to_string(),
                            deleted: current_deleted,
                            envelope: String::from_utf8(plaintext).map_err(|_| {
                                AppError::internal("stored item envelope is not valid utf-8")
                            })?,
                            updated_at: Some(current_updated_at.to_rfc3339()),
                        }),
                    });
                }
            }
        }
    }

    Ok(Json(PushResponse { results }))
}
