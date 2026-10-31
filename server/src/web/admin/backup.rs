//! The admin page's Backups tab (`/admin/api/backup*`). Secrets (the S3
//! secret key and the backup passphrase) are never returned, only whether
//! they are set, like the OIDC client secret.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{clean, internal, Admin, AdminCtx};
use crate::backup::{self, BackupConfig, Outcome, Trigger, MIN_PASSPHRASE_LEN};
use crate::error::{AppError, AppResult};
use crate::settings::{self, FileBackup, FileConfig};

pub(super) fn routes() -> Router<AdminCtx> {
    Router::new()
        .route("/backup", get(get_backup).put(put_backup))
        .route("/backup/test", post(test_connection))
        .route("/backup/run", post(run_now))
        .route("/backup/list", get(list))
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct BackupView {
    enabled: bool,
    endpoint: Option<String>,
    region: Option<String>,
    bucket: Option<String>,
    prefix: Option<String>,
    access_key_id: Option<String>,
    /// Never returned; on save, empty keeps the stored one.
    #[serde(skip_serializing)]
    secret_access_key: Option<String>,
    secret_access_key_set: bool,
    path_style: bool,
    /// Never returned; on save, empty keeps the stored one.
    #[serde(skip_serializing)]
    passphrase: Option<String>,
    passphrase_set: bool,
    interval_hours: u32,
    keep: u32,
}

async fn effective(ctx: &AdminCtx) -> AppResult<BackupConfig> {
    BackupConfig::load(&ctx.store()).await.map_err(internal)
}

fn backup_keys(keys: &[&'static str]) -> Vec<&'static str> {
    keys.iter()
        .copied()
        .filter(|k| k.starts_with("backup_"))
        .collect()
}

/// Backup keys set by the environment or `config.toml` (read-only here).
fn locked_keys(ctx: &AdminCtx) -> Vec<&'static str> {
    let e = &ctx.effective;
    backup_keys(&[e.env_locked.as_slice(), e.file_locked.as_slice()].concat())
}

fn status(ctx: &AdminCtx, cfg: &BackupConfig) -> serde_json::Value {
    let st = ctx.backups.state();
    json!({
        "enabled": cfg.enabled,
        "running": ctx.backups.running(),
        "next_check": ctx.backups.next_check().filter(|_| cfg.enabled),
        "last_success_at": st.last_success_at,
        "last_size": st.last_size,
        "last_key": st.last_key,
        "last_check_at": st.last_check_at,
        "last_result": st.last_result,
        "last_error": st.last_error,
        "last_error_at": st.last_error_at,
    })
}

async fn get_backup(State(ctx): State<AdminCtx>, _: Admin) -> AppResult<Json<serde_json::Value>> {
    let c = effective(&ctx).await?;
    let view = BackupView {
        enabled: c.enabled,
        endpoint: Some(c.s3.endpoint.clone()).filter(|s| !s.is_empty()),
        region: Some(c.s3.region.clone()),
        bucket: Some(c.s3.bucket.clone()).filter(|s| !s.is_empty()),
        prefix: Some(c.prefix.clone()).filter(|s| !s.is_empty()),
        access_key_id: Some(c.s3.access_key_id.clone()).filter(|s| !s.is_empty()),
        secret_access_key: None,
        secret_access_key_set: !c.s3.secret_access_key.is_empty(),
        path_style: c.s3.path_style,
        passphrase: None,
        passphrase_set: !c.passphrase.is_empty(),
        interval_hours: c.interval_hours,
        keep: c.keep,
    };
    Ok(Json(json!({
        "settings": view,
        "env_locked": backup_keys(&ctx.effective.env_locked),
        "file_locked": backup_keys(&ctx.effective.file_locked),
        "status": status(&ctx, &c),
        "min_passphrase_len": MIN_PASSPHRASE_LEN,
    })))
}

/// The form merged into the stored `[backup]` table, skipping env-locked
/// keys and keeping stored secrets when the form leaves them empty.
fn merge(stored: Option<FileBackup>, new: BackupView, locked: &[&str]) -> AppResult<FileBackup> {
    let mut b = stored.unwrap_or_default();
    let is_locked = |k: &str| locked.iter().any(|l| l.strip_prefix("backup_") == Some(k));
    if let Some(p) = new.passphrase.as_deref().filter(|p| !p.is_empty()) {
        if p.chars().count() < MIN_PASSPHRASE_LEN {
            return Err(AppError::bad_request(
                "weak_passphrase",
                format!("the backup passphrase needs at least {MIN_PASSPHRASE_LEN} characters"),
            ));
        }
    }
    if new.interval_hours == 0 {
        return Err(AppError::bad_request(
            "invalid_interval",
            "check at least every 1 hour",
        ));
    }
    if let Some(e) = clean(new.endpoint.clone()) {
        if !(e.starts_with("http://") || e.starts_with("https://")) {
            return Err(AppError::bad_request(
                "invalid_endpoint",
                "the endpoint must start with http:// or https://",
            ));
        }
    }
    if !is_locked("enabled") {
        b.enabled = Some(new.enabled);
    }
    if !is_locked("endpoint") {
        b.endpoint = clean(new.endpoint);
    }
    if !is_locked("region") {
        b.region = clean(new.region);
    }
    if !is_locked("bucket") {
        b.bucket = clean(new.bucket);
    }
    if !is_locked("prefix") {
        b.prefix = clean(new.prefix);
    }
    if !is_locked("access_key_id") {
        b.access_key_id = clean(new.access_key_id);
    }
    if !is_locked("secret_access_key") {
        if let Some(s) = new.secret_access_key.filter(|s| !s.trim().is_empty()) {
            b.secret_access_key = Some(s.trim().to_string());
        }
    }
    if !is_locked("path_style") {
        b.path_style = Some(new.path_style);
    }
    if !is_locked("passphrase") {
        if let Some(p) = new.passphrase.filter(|p| !p.is_empty()) {
            b.passphrase = Some(p);
        }
    }
    if !is_locked("interval_hours") {
        b.interval_hours = Some(new.interval_hours);
    }
    if !is_locked("keep") {
        b.keep = Some(new.keep);
    }
    Ok(b)
}

/// The database's settings with the form applied, and what the backup
/// settings would then be (environment and `config.toml` included).
async fn preview(ctx: &AdminCtx, new: BackupView) -> AppResult<(FileConfig, BackupConfig)> {
    let store = ctx.store();
    let mut db = store.read_db().await.map_err(internal)?;
    db.backup = Some(merge(db.backup.take(), new, &locked_keys(ctx))?);
    let file = settings::read(&ctx.data_dir).map_err(internal)?;
    let eff = settings::resolve(&file, Some(&db), &settings::process_env).map_err(internal)?;
    let cfg = BackupConfig::from_file(eff.values.backup.as_ref());
    Ok((db, cfg))
}

async fn put_backup(
    State(ctx): State<AdminCtx>,
    _: Admin,
    Json(new): Json<BackupView>,
) -> AppResult<Json<serde_json::Value>> {
    let (db, cfg) = preview(&ctx, new).await?;
    if cfg.enabled {
        cfg.check()
            .map_err(|e| AppError::bad_request("incomplete_backup_settings", e.to_string()))?;
    }
    ctx.store().write_db(&db).await.map_err(internal)?;
    tracing::info!("admin page changed backup settings; reloading");
    // Restarts the schedule, so the first check comes ~2 minutes from now.
    ctx.reload.trigger_soon();
    Ok(Json(json!({ "ok": true, "reloading": true })))
}

async fn test_connection(
    State(ctx): State<AdminCtx>,
    _: Admin,
    Json(new): Json<BackupView>,
) -> AppResult<Json<serde_json::Value>> {
    let (_, cfg) = preview(&ctx, new).await?;
    let client = cfg
        .client()
        .map_err(|e| AppError::bad_request("incomplete_backup_settings", e.to_string()))?;
    client
        .test(&cfg.prefix)
        .await
        .map_err(|e| AppError::bad_request("backup_connection_failed", format!("{e:#}")))?;
    Ok(Json(json!({
        "ok": true,
        "message": format!("Connected to bucket {}.", cfg.s3.bucket),
    })))
}

async fn run_now(State(ctx): State<AdminCtx>, _: Admin) -> AppResult<Json<serde_json::Value>> {
    match ctx.backups.run(&ctx.store(), Trigger::Manual).await {
        Ok(Outcome::Busy) => Err(AppError::new(
            StatusCode::CONFLICT,
            "backup_running",
            "a backup is already running",
        )),
        Ok(outcome) => {
            let cfg = effective(&ctx).await?;
            Ok(Json(
                json!({ "outcome": outcome, "status": status(&ctx, &cfg) }),
            ))
        }
        Err(e) => Err(AppError::new(
            StatusCode::BAD_GATEWAY,
            "backup_failed",
            format!("{e:#}"),
        )),
    }
}

async fn list(State(ctx): State<AdminCtx>, _: Admin) -> AppResult<Json<serde_json::Value>> {
    let cfg = effective(&ctx).await?;
    let client = cfg
        .client()
        .map_err(|e| AppError::bad_request("incomplete_backup_settings", e.to_string()))?;
    let mut objects: Vec<_> = client
        .list(&cfg.prefix)
        .await
        .map_err(|e| {
            AppError::new(
                StatusCode::BAD_GATEWAY,
                "backup_list_failed",
                format!("{e:#}"),
            )
        })?
        .into_iter()
        .filter(|o| {
            o.key
                .strip_prefix(cfg.prefix.as_str())
                .is_some_and(backup::format::is_backup_name)
        })
        .collect();
    objects.sort_by(|a, b| b.key.cmp(&a.key));
    Ok(Json(json!({ "bucket": cfg.s3.bucket, "backups": objects })))
}
