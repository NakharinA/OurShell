//! Small, shared database helpers used by several route modules. Everything
//! here uses runtime `sqlx::query`/`query_as` (never the `query!` macros),
//! so no `DATABASE_URL` is needed at compile time.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::crypto;
use crate::error::AppError;
use crate::sqlx;
use crate::sqlx::PgPool;

pub fn parse_uuid(s: &str, field: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(s)
        .map_err(|_| AppError::bad_request("invalid_id", format!("{field} is not a valid id")))
}

pub async fn total_user_count(pool: &PgPool) -> sqlx::Result<i64> {
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await?;
    Ok(count)
}

/// The single org every user currently belongs to (phase 5 only ever
/// creates one). `None` until the very first user registers.
pub async fn default_org_id(pool: &PgPool) -> sqlx::Result<Option<Uuid>> {
    let row: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM orgs ORDER BY created_at ASC LIMIT 1")
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|r| r.0))
}

pub async fn create_org(pool: &PgPool, name: &str) -> sqlx::Result<Uuid> {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO orgs (id, name) VALUES ($1, $2)")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await?;
    Ok(id)
}

pub async fn create_session(
    pool: &PgPool,
    org_id: Uuid,
    user_id: Uuid,
    device_id: Uuid,
    ttl_hours: i64,
) -> sqlx::Result<String> {
    let token = crypto::random_token();
    let hash = crypto::hash_token(&token);
    let id = Uuid::new_v4();
    let expires_at: DateTime<Utc> = Utc::now() + chrono::Duration::hours(ttl_hours.max(1));
    sqlx::query(
        "INSERT INTO sessions (id, org_id, user_id, device_id, token_hash, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(org_id)
    .bind(user_id)
    .bind(device_id)
    .bind(&hash)
    .bind(expires_at)
    .execute(pool)
    .await?;
    Ok(token)
}

pub async fn log_event(
    pool: &PgPool,
    org_id: Uuid,
    user_id: Option<Uuid>,
    kind: &str,
    detail: Option<&str>,
) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO events (org_id, user_id, kind, detail) VALUES ($1, $2, $3, $4)")
        .bind(org_id)
        .bind(user_id)
        .bind(kind)
        .bind(detail)
        .execute(pool)
        .await?;
    Ok(())
}

/// Registers a new device or refreshes an existing one's name/public key.
/// Enforces `max_devices` (0 = unlimited) and refuses a revoked device id.
pub async fn upsert_device(
    pool: &PgPool,
    org_id: Uuid,
    user_id: Uuid,
    device_id: Uuid,
    name: &str,
    public_key: &str,
    max_devices: u32,
) -> Result<(), AppError> {
    let existing: Option<(bool,)> =
        sqlx::query_as("SELECT revoked FROM devices WHERE id = $1 AND user_id = $2")
            .bind(device_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;

    match existing {
        Some((true,)) => Err(AppError::forbidden(
            "device_revoked",
            "this device was revoked and cannot be reused",
        )),
        Some((false,)) => {
            sqlx::query("UPDATE devices SET name = $1, public_key = $2 WHERE id = $3")
                .bind(name)
                .bind(public_key)
                .bind(device_id)
                .execute(pool)
                .await?;
            Ok(())
        }
        None => {
            let (count,): (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM devices WHERE user_id = $1 AND revoked = false",
            )
            .bind(user_id)
            .fetch_one(pool)
            .await?;
            if max_devices > 0 && count as u32 >= max_devices {
                return Err(AppError::forbidden(
                    "too_many_devices",
                    "device limit reached for this account",
                ));
            }
            sqlx::query(
                "INSERT INTO devices (id, org_id, user_id, name, public_key) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(device_id)
            .bind(org_id)
            .bind(user_id)
            .bind(name)
            .bind(public_key)
            .execute(pool)
            .await?;
            Ok(())
        }
    }
}

/// Checks that `user_id` is an (enabled) member of `org_id`.
pub async fn user_in_org(pool: &PgPool, user_id: Uuid, org_id: Uuid) -> sqlx::Result<bool> {
    let row: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM users WHERE id = $1 AND org_id = $2")
        .bind(user_id)
        .bind(org_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

pub async fn is_vault_member(pool: &PgPool, vault_id: Uuid, user_id: Uuid) -> sqlx::Result<bool> {
    let row: Option<(Uuid,)> =
        sqlx::query_as("SELECT vault_id FROM vault_members WHERE vault_id = $1 AND user_id = $2")
            .bind(vault_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.is_some())
}

/// Every vault (personal and team) `user_id` currently belongs to, as
/// `(vault_id, kind, name, key_id, sealed_vault_key, recovery_wrap)`.
/// Used both by `login::finish` (building the initial `SessionResponse`)
/// and `account::vaults` (letting an already-signed-in session refresh
/// its list, e.g. after being added to a team vault).
pub type VaultRow = (String, String, String, String, Vec<u8>, Option<Vec<u8>>);

pub async fn user_vaults(pool: &PgPool, user_id: Uuid) -> sqlx::Result<Vec<VaultRow>> {
    sqlx::query_as(
        r#"
        SELECT v.id::text, v.kind, v.name, vm.key_id, vm.sealed_vault_key, vm.recovery_wrap
        FROM vault_members vm
        JOIN vaults v ON v.id = vm.vault_id
        WHERE vm.user_id = $1
        ORDER BY v.kind DESC, v.name
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}
