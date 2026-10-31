//! First-run setup: what the server serves while it has no database URL or
//! master key. Every API call needs the one-time setup code (printed in the
//! server log, and shown by the installer); finishing writes `config.toml`
//! and reloads the server into normal mode.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::middleware;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use ourshell_proto::sync::ApiError;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{password, Reload, VERSION};
use crate::config;
use crate::error::{AppError, AppResult};
use crate::proxy::ProxyTrust;
use crate::rate_limit::{limit_with, RateLimiter};
use crate::settings::{self, Effective, FileTls};
use crate::sqlx::PgPoolOptions;

#[derive(Clone)]
pub struct SetupCtx {
    pub data_dir: PathBuf,
    pub code: String,
    /// Settings as loaded (file + environment): tells the wizard what the
    /// installer already provided.
    pub effective: Arc<Effective>,
    pub reload: Reload,
    done: Arc<AtomicBool>,
}

impl SetupCtx {
    pub fn new(data_dir: PathBuf, code: String, effective: Effective, reload: Reload) -> Self {
        Self {
            data_dir,
            code,
            effective: Arc::new(effective),
            reload,
            done: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Proxy mode as configured so far (usually by the installer), so the
    /// wizard's own rate limit sees real client addresses too.
    fn proxy_trust(&self) -> ProxyTrust {
        let v = &self.effective.values;
        if !v.behind_proxy.unwrap_or(false) {
            return ProxyTrust::off();
        }
        ProxyTrust::behind_proxy(v.trusted_proxies.as_deref().unwrap_or_default()).unwrap_or_else(
            |e| {
                tracing::warn!(error = %e, "ignoring invalid trusted proxies during setup");
                ProxyTrust::behind_proxy(&[]).unwrap_or_default()
            },
        )
    }

    fn env_database_url(&self) -> Option<&str> {
        self.effective
            .env_locked
            .contains(&"database_url")
            .then_some(self.effective.values.database_url.as_deref())
            .flatten()
    }
}

pub fn router(ctx: SetupCtx) -> Router {
    let api = Router::new()
        .route("/verify", post(verify))
        .route("/test-db", post(test_db))
        .route("/finish", post(finish))
        .route_layer(middleware::from_fn_with_state(
            Arc::new(RateLimiter::new(20).with_proxy(ctx.proxy_trust())),
            limit_with,
        ))
        .route("/info", get(info));
    Router::new()
        .route("/", get(|| async { Redirect::to("/setup") }))
        .route("/admin", get(|| async { Redirect::to("/setup") }))
        .route("/setup", get(|| async { super::page(super::SETUP_HTML) }))
        .nest("/setup/api", api)
        .route(
            "/v1/health",
            get(|| async { Json(json!({ "status": "ok", "mode": "setup", "version": VERSION })) }),
        )
        .route("/v1/{*rest}", any(not_set_up))
        .merge(super::assets())
        .with_state(ctx)
}

async fn not_set_up() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ApiError {
            error: "not_set_up".into(),
            message: "this server hasn't been set up yet: open /setup on it in a browser".into(),
        }),
    )
        .into_response()
}

fn error(status: StatusCode, code: &'static str, message: impl Into<String>) -> AppError {
    AppError::new(status, code, message)
}

fn check_code(ctx: &SetupCtx, code: &str) -> AppResult<()> {
    if ctx.done.load(Ordering::SeqCst) {
        return Err(error(
            StatusCode::CONFLICT,
            "already_set_up",
            "setup is already complete",
        ));
    }
    if settings::setup_code_matches(&ctx.code, code) {
        Ok(())
    } else {
        Err(error(
            StatusCode::UNAUTHORIZED,
            "invalid_setup_code",
            "that setup code is not correct; it is printed in the server log",
        ))
    }
}

async fn info(State(ctx): State<SetupCtx>) -> Json<serde_json::Value> {
    Json(json!({
        "version": VERSION,
        "database_from_env": ctx.env_database_url().is_some(),
        "database": ctx.env_database_url().map(super::redact_database_url),
        "tls_domain": ctx.effective.values.tls.as_ref().map(|t| t.domain.clone()).filter(|d| !d.is_empty()),
        "public_url": ctx.effective.values.public_url,
        "behind_proxy": ctx.effective.values.behind_proxy.unwrap_or(false),
        "behind_proxy_from_env": ctx.effective.env_locked.contains(&"behind_proxy"),
    }))
}

#[derive(Deserialize)]
struct CodeOnly {
    code: String,
}

async fn verify(
    State(ctx): State<SetupCtx>,
    Json(body): Json<CodeOnly>,
) -> AppResult<Json<serde_json::Value>> {
    check_code(&ctx, &body.code)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct TestDb {
    code: String,
    database_url: Option<String>,
}

/// Connects to `url` and returns the server's version string.
pub(crate) async fn probe_database(url: &str) -> Result<String, String> {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .connect(url)
        .await
        .map_err(|e| e.to_string())?;
    let (version,): (String,) = crate::sqlx::query_as("SELECT version()")
        .fetch_one(&pool)
        .await
        .map_err(|e| e.to_string())?;
    pool.close().await;
    Ok(version)
}

fn pick_database_url(ctx: &SetupCtx, given: Option<String>) -> AppResult<String> {
    if let Some(url) = ctx.env_database_url() {
        return Ok(url.to_string());
    }
    given
        .map(|u| u.trim().to_string())
        .filter(|u| u.starts_with("postgres://") || u.starts_with("postgresql://"))
        .ok_or_else(|| {
            error(
                StatusCode::BAD_REQUEST,
                "invalid_database_url",
                "enter a PostgreSQL URL like postgres://user:password@host:5432/ourshell",
            )
        })
}

async fn test_db(
    State(ctx): State<SetupCtx>,
    Json(body): Json<TestDb>,
) -> AppResult<Json<serde_json::Value>> {
    check_code(&ctx, &body.code)?;
    let url = pick_database_url(&ctx, body.database_url)?;
    Ok(Json(match probe_database(&url).await {
        Ok(version) => json!({ "ok": true, "version": version }),
        Err(e) => json!({ "ok": false, "error": e }),
    }))
}

#[derive(Deserialize)]
struct FinishRequest {
    code: String,
    database_url: Option<String>,
    public_url: Option<String>,
    tls_domain: Option<String>,
    tls_email: Option<String>,
    /// A reverse proxy / ingress handles HTTPS (excludes `tls_domain`).
    #[serde(default)]
    behind_proxy: bool,
    signups_enabled: bool,
    invite_only: bool,
    max_devices_per_user: Option<u32>,
    admin_password: String,
}

#[derive(Serialize)]
struct FinishResponse {
    /// The generated `server_master_key`, or `None` if the environment
    /// provides one.
    master_key: Option<String>,
    /// `config.toml` as written, offered as a backup download.
    config_toml: String,
}

fn clean(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

async fn finish(
    State(ctx): State<SetupCtx>,
    Json(body): Json<FinishRequest>,
) -> AppResult<Json<FinishResponse>> {
    check_code(&ctx, &body.code)?;
    if body.admin_password.chars().count() < password::MIN_LEN {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "weak_password",
            format!(
                "the admin password needs at least {} characters",
                password::MIN_LEN
            ),
        ));
    }
    let url = pick_database_url(&ctx, body.database_url)?;
    if let Err(e) = probe_database(&url).await {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "database_unreachable",
            format!("can't connect to the database: {e}"),
        ));
    }
    let public_url = clean(body.public_url);
    if let Some(u) = &public_url {
        if !(u.starts_with("http://") || u.starts_with("https://")) {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "invalid_public_url",
                "the server address must start with http:// or https://",
            ));
        }
    }
    let hash =
        password::hash(&body.admin_password).map_err(|e| AppError::internal(e.to_string()))?;
    let mut file = settings::read(&ctx.data_dir).map_err(|e| AppError::internal(e.to_string()))?;
    if ctx.env_database_url().is_none() {
        file.database_url = Some(url);
    }
    let env_has_key = ctx.effective.env_locked.contains(&"server_master_key");
    let master_key = if env_has_key {
        None
    } else {
        let key = file
            .server_master_key
            .clone()
            .unwrap_or_else(config::generate_master_key);
        file.server_master_key = Some(key.clone());
        Some(key)
    };
    file.public_url = public_url;
    if !ctx.effective.env_locked.contains(&"behind_proxy") {
        file.behind_proxy = Some(body.behind_proxy);
    }
    let behind_proxy = ctx
        .effective
        .values
        .behind_proxy
        .filter(|_| ctx.effective.env_locked.contains(&"behind_proxy"))
        .unwrap_or(body.behind_proxy);
    file.tls = clean(body.tls_domain)
        .filter(|_| !behind_proxy)
        .map(|domain| FileTls {
            domain,
            email: clean(body.tls_email),
        });
    file.signups_enabled = Some(body.signups_enabled);
    file.invite_only = Some(body.invite_only);
    file.max_devices_per_user = body.max_devices_per_user;
    file.admin_password_hash = Some(hash);

    let config_toml = settings::write(&ctx.data_dir, &file)
        .and_then(|()| settings::to_toml(&file))
        .map_err(|e| {
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "write_failed",
                format!("couldn't save the configuration: {e:#}"),
            )
        })?;
    let _ = std::fs::remove_file(ctx.data_dir.join(settings::SETUP_CODE_FILE));
    ctx.done.store(true, Ordering::SeqCst);
    tracing::info!("setup complete; reloading into normal mode");
    ctx.reload.trigger_soon();
    Ok(Json(FinishResponse {
        master_key,
        config_toml,
    }))
}
