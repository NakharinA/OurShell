//! Setup mode: what the server serves until it has a database with its
//! data and the master key for it (see [`crate::startup`] for when). Every
//! API call but `info` needs the one-time setup code (printed in the
//! server log, and shown by the installer).
//!
//! - `finish` sets up a new server in an empty database: runs the
//!   migrations, stores the settings (encrypted) in the database, and the
//!   database URL / master key in `config.toml` when the environment
//!   doesn't provide them.
//! - `inspect` shows what a database holds; for one with an OurShell
//!   server's data, `import` (with the original master key) takes it over
//!   as is, or `clear` (confirmed by typing DELETE) drops it first.
//!
//! Each of them reloads the server into normal mode when done.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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
use crate::settings::{self, Effective, FileConfig, FileTls};
use crate::sqlx::PgPool;
use crate::startup::{self, Inspection, KeyStatus, Remembered, SetupState};

/// What the user must type to clear a database.
pub const CLEAR_CONFIRMATION: &str = "DELETE";

#[derive(Clone)]
pub struct SetupCtx {
    pub data_dir: PathBuf,
    pub code: String,
    /// Settings as loaded (file + environment): tells the wizard what the
    /// installer already provided.
    pub effective: Arc<Effective>,
    pub reload: Reload,
    /// Why the server is in setup mode.
    pub startup: Arc<SetupState>,
    /// Bootstrap keys kept in memory when `config.toml` can't hold them.
    pub remembered: Remembered,
    done: Arc<AtomicBool>,
}

impl SetupCtx {
    pub fn new(data_dir: PathBuf, code: String, effective: Effective, reload: Reload) -> Self {
        let boot = bootstrap_of(&effective);
        Self {
            data_dir,
            code,
            effective: Arc::new(effective),
            reload,
            startup: Arc::new(SetupState::no_database(&boot)),
            remembered: Remembered::default(),
            done: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The startup decision's details, and the process's remembered keys.
    pub fn with_startup(mut self, state: SetupState, remembered: Remembered) -> Self {
        self.startup = Arc::new(state);
        self.remembered = remembered;
        self
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

    fn key_from_env(&self) -> bool {
        self.effective.env_locked.contains(&"server_master_key")
    }

    /// The master key known without asking: environment, `config.toml`,
    /// or remembered from earlier in this process.
    fn known_key(&self) -> Option<String> {
        self.effective
            .values
            .server_master_key
            .clone()
            .or_else(|| self.remembered.get().server_master_key)
            .filter(|k| !k.trim().is_empty())
    }

    /// The database URL known without asking (environment, `config.toml`,
    /// remembered).
    fn known_database_url(&self) -> Option<String> {
        self.effective
            .values
            .database_url
            .clone()
            .or_else(|| self.remembered.get().database_url)
            .filter(|u| !u.trim().is_empty())
    }
}

fn bootstrap_of(effective: &Effective) -> startup::Bootstrap {
    startup::Bootstrap {
        database_url: effective.values.database_url.clone(),
        url_from_env: effective.env_locked.contains(&"database_url"),
        master_key: effective.values.server_master_key.clone(),
        key_from_env: effective.env_locked.contains(&"server_master_key"),
        effective: effective.clone(),
    }
}

pub fn router(ctx: SetupCtx) -> Router {
    let api = Router::new()
        .route("/verify", post(verify))
        .route("/status", post(status))
        .route("/test-db", post(test_db))
        .route("/inspect", post(inspect))
        .route("/import", post(import))
        .route("/clear", post(clear))
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

fn internal(e: anyhow::Error) -> AppError {
    AppError::internal(format!("{e:#}"))
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
        "phase": ctx.startup.phase,
        "database_from_env": ctx.env_database_url().is_some(),
        "database": ctx.env_database_url().map(super::redact_database_url),
        "key_from_env": ctx.key_from_env(),
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

/// Why the server is in setup mode, with details (the connection error,
/// what the database holds) only for someone with the setup code.
async fn status(
    State(ctx): State<SetupCtx>,
    Json(body): Json<CodeOnly>,
) -> AppResult<Json<SetupState>> {
    check_code(&ctx, &body.code)?;
    Ok(Json((*ctx.startup).clone()))
}

#[derive(Deserialize)]
struct TestDb {
    code: String,
    database_url: Option<String>,
}

/// Connects to `url` and returns the server's version string.
pub(crate) async fn probe_database(url: &str) -> Result<String, String> {
    let pool = startup::connect_once(url).await?;
    let (version,): (String,) = crate::sqlx::query_as("SELECT version()")
        .fetch_one(&pool)
        .await
        .map_err(|e| startup::redact_error(&e.to_string(), url))?;
    pool.close().await;
    Ok(version)
}

fn pick_database_url(ctx: &SetupCtx, given: Option<String>) -> AppResult<String> {
    if let Some(url) = ctx.env_database_url() {
        return Ok(url.to_string());
    }
    let given = given
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty());
    let Some(url) = given.or_else(|| ctx.known_database_url()) else {
        return Err(invalid_url());
    };
    if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        Ok(url)
    } else {
        Err(invalid_url())
    }
}

fn invalid_url() -> AppError {
    error(
        StatusCode::BAD_REQUEST,
        "invalid_database_url",
        "enter a PostgreSQL URL like postgres://user:password@host:5432/ourshell",
    )
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

async fn connect(url: &str) -> AppResult<PgPool> {
    startup::connect_once(url).await.map_err(|e| {
        error(
            StatusCode::BAD_REQUEST,
            "database_unreachable",
            format!("can't connect to the database: {e}"),
        )
    })
}

/// The key status of the known master key (if any) for this database.
async fn known_key_status(ctx: &SetupCtx, pool: &PgPool, insp: &Inspection) -> KeyStatus {
    let Some(key) = ctx.known_key() else {
        return KeyStatus::Missing;
    };
    if insp.kind.is_new() {
        return KeyStatus::Unverifiable;
    }
    match config::decode_master_key(&key) {
        Ok(k) => startup::check_key(pool, &k)
            .await
            .unwrap_or(KeyStatus::Wrong),
        Err(_) => KeyStatus::Wrong,
    }
}

#[derive(Serialize)]
struct InspectResponse {
    ok: bool,
    error: Option<String>,
    database: Option<String>,
    database_from_env: bool,
    inspection: Option<Inspection>,
    /// Whether the configured master key (environment / `config.toml`)
    /// opens this database.
    key_status: KeyStatus,
    key_from_env: bool,
    /// What to type to confirm `clear`.
    clear_confirmation: &'static str,
}

/// Connects and reports what the database holds (read-only).
async fn inspect(
    State(ctx): State<SetupCtx>,
    Json(body): Json<TestDb>,
) -> AppResult<Json<InspectResponse>> {
    check_code(&ctx, &body.code)?;
    let url = pick_database_url(&ctx, body.database_url)?;
    let mut resp = InspectResponse {
        ok: false,
        error: None,
        database: Some(super::redact_database_url(&url)),
        database_from_env: ctx.env_database_url().is_some(),
        inspection: None,
        key_status: KeyStatus::Missing,
        key_from_env: ctx.key_from_env(),
        clear_confirmation: CLEAR_CONFIRMATION,
    };
    let pool = match startup::connect_once(&url).await {
        Ok(p) => p,
        Err(e) => {
            resp.error = Some(e);
            return Ok(Json(resp));
        }
    };
    let insp = startup::inspect(&pool).await.map_err(internal)?;
    resp.key_status = known_key_status(&ctx, &pool, &insp).await;
    pool.close().await;
    resp.ok = true;
    resp.inspection = Some(insp);
    Ok(Json(resp))
}

#[derive(Deserialize)]
struct ClearRequest {
    code: String,
    database_url: Option<String>,
    confirm: Option<String>,
}

/// Drops every OurShell table so the database can be set up anew.
async fn clear(
    State(ctx): State<SetupCtx>,
    Json(body): Json<ClearRequest>,
) -> AppResult<Json<serde_json::Value>> {
    check_code(&ctx, &body.code)?;
    if body.confirm.as_deref().map(str::trim) != Some(CLEAR_CONFIRMATION) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "confirmation_required",
            format!("type {CLEAR_CONFIRMATION} to confirm deleting everything in the database"),
        ));
    }
    let url = pick_database_url(&ctx, body.database_url)?;
    let pool = connect(&url).await?;
    let dropped = startup::clear(&pool).await.map_err(internal)?;
    let insp = startup::inspect(&pool).await.map_err(internal)?;
    pool.close().await;
    tracing::warn!(
        tables = dropped.len(),
        "the setup wizard cleared the database at {}",
        super::redact_database_url(&url)
    );
    Ok(Json(
        json!({ "ok": true, "dropped": dropped, "inspection": insp }),
    ))
}

/// What the final screen shows.
#[derive(Serialize)]
struct DoneResponse {
    /// The master key, when the environment doesn't provide it (generated
    /// by `finish`, or the one `import` was given).
    master_key: Option<String>,
    /// `config.toml` as written (`None` if it couldn't be).
    config_toml: Option<String>,
    /// The data dir survives a redeploy; otherwise `env_lines` must go into
    /// the environment.
    persistent: bool,
    persistence: String,
    /// `SERVER_MASTER_KEY=...` / `DATABASE_URL=...` lines to add to the
    /// environment so a redeploy keeps working (empty when not needed).
    env_lines: Vec<String>,
    database_from_env: bool,
    key_from_env: bool,
    /// Import: accounts that were kept.
    accounts: Option<i64>,
}

/// Saves the bootstrap keys (in `config.toml` when the data dir is
/// writable, and in memory either way) and builds the final screen.
fn save_bootstrap(
    ctx: &SetupCtx,
    url: &str,
    key_b64: &str,
    strip_settings: bool,
    master_key_to_show: Option<String>,
    accounts: Option<i64>,
) -> DoneResponse {
    let url_from_env = ctx.env_database_url().is_some();
    let key_from_env = ctx.key_from_env();
    let boot = FileConfig {
        database_url: (!url_from_env).then(|| url.to_string()),
        server_master_key: (!key_from_env).then(|| key_b64.to_string()),
        ..Default::default()
    };
    ctx.remembered.set(&boot);
    let mut persistence = startup::data_dir_persistence(&ctx.data_dir);
    let config_toml = if persistence.writable {
        match settings::update_bootstrap(&ctx.data_dir, &boot, strip_settings) {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), "couldn't write config.toml");
                persistence.persistent = false;
                persistence.detail = format!("config.toml couldn't be written: {e:#}");
                None
            }
        }
    } else {
        None
    };
    let env_lines = if persistence.persistent {
        Vec::new()
    } else {
        startup::env_lines(Some(url), url_from_env, Some(key_b64), key_from_env)
    };
    if !env_lines.is_empty() {
        tracing::warn!(
            "{}: add SERVER_MASTER_KEY{} to the environment before the next redeploy",
            persistence.detail,
            if url_from_env {
                ""
            } else {
                " and DATABASE_URL"
            }
        );
    }
    DoneResponse {
        master_key: master_key_to_show,
        config_toml,
        persistent: persistence.persistent,
        persistence: persistence.detail,
        env_lines,
        database_from_env: url_from_env,
        key_from_env,
        accounts,
    }
}

fn complete(ctx: &SetupCtx) {
    let _ = std::fs::remove_file(ctx.data_dir.join(settings::SETUP_CODE_FILE));
    ctx.done.store(true, Ordering::SeqCst);
    ctx.reload.trigger_soon();
}

fn check_password(pw: &str) -> AppResult<()> {
    if pw.chars().count() < password::MIN_LEN {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "weak_password",
            format!(
                "the admin password needs at least {} characters",
                password::MIN_LEN
            ),
        ));
    }
    Ok(())
}

async fn migrate(pool: &PgPool) -> AppResult<()> {
    crate::migrator()
        .await
        .map_err(internal)?
        .run(pool)
        .await
        .map_err(|e| {
            error(
                StatusCode::CONFLICT,
                "migration_failed",
                format!(
                    "couldn't create OurShell's tables ({e}); if the database has leftovers of an \
                     earlier OurShell server, clear it first"
                ),
            )
        })
}

#[derive(Deserialize)]
struct ImportRequest {
    code: String,
    database_url: Option<String>,
    /// The original master key; empty = the configured one.
    master_key: Option<String>,
    /// Only needed when no admin password is stored anywhere yet.
    admin_password: Option<String>,
}

/// Takes over a database with an OurShell server's data: checks the
/// master key against it, migrates it to this version, and keeps every
/// account and setting.
async fn import(
    State(ctx): State<SetupCtx>,
    Json(body): Json<ImportRequest>,
) -> AppResult<Json<DoneResponse>> {
    check_code(&ctx, &body.code)?;
    let url = pick_database_url(&ctx, body.database_url)?;
    let given = body
        .master_key
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty());
    let key_b64 =
        match (given, ctx.key_from_env()) {
            (Some(_), true) => return Err(error(
                StatusCode::BAD_REQUEST,
                "master_key_from_env",
                "SERVER_MASTER_KEY is set in the environment; change it there and restart instead",
            )),
            (Some(k), false) => k,
            (None, _) => ctx.known_key().ok_or_else(|| {
                error(
                    StatusCode::BAD_REQUEST,
                    "master_key_required",
                    "enter the master key of the server that created this data",
                )
            })?,
        };
    let key = config::decode_master_key(&key_b64).map_err(|_| {
        error(
            StatusCode::BAD_REQUEST,
            "invalid_master_key",
            "that's not a master key: it's 44 characters of base64 (32 bytes)",
        )
    })?;
    let pool = connect(&url).await?;
    let insp = startup::inspect(&pool).await.map_err(internal)?;
    if insp.kind.is_new() {
        return Err(error(
            StatusCode::CONFLICT,
            "no_existing_data",
            "this database has no OurShell data to import; continue with a new setup instead",
        ));
    }
    match startup::check_key(&pool, &key).await.map_err(internal)? {
        KeyStatus::Wrong => {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "wrong_master_key",
                if ctx.key_from_env() {
                    "SERVER_MASTER_KEY in the environment doesn't match this database: set it to \
                     the original key and restart, or clear the database"
                } else {
                    "this master key doesn't match the database: it must be the key of the \
                     server that created this data"
                },
            ))
        }
        KeyStatus::Ok | KeyStatus::Unverifiable | KeyStatus::Missing => {}
    }
    migrate(&pool).await?;
    let file = settings::read(&ctx.data_dir).map_err(internal)?;
    let stored = settings::db::read(&pool, &key).await.map_err(internal)?;
    // A 0.8.x config.toml's settings move into the database (once).
    let had_row = stored.is_some();
    let mut db = stored.unwrap_or_else(|| file.without_bootstrap());
    let eff = settings::resolve(&file, Some(&db), &settings::process_env).map_err(internal)?;
    let mut changed = !had_row && db.has_settings();
    if eff.values.admin_password_hash.is_none() {
        let Some(pw) = body.admin_password.filter(|p| !p.is_empty()) else {
            return Err(error(
                StatusCode::CONFLICT,
                "admin_password_required",
                "this database has no admin password yet; choose one",
            ));
        };
        check_password(&pw)?;
        db.admin_password_hash = Some(password::hash(&pw).map_err(internal)?);
        changed = true;
    }
    if changed {
        settings::db::write(&pool, &key, &db)
            .await
            .map_err(internal)?;
    }
    pool.close().await;
    let shown = (!ctx.key_from_env()).then(|| key_b64.clone());
    let done = save_bootstrap(&ctx, &url, &key_b64, !had_row, shown, Some(insp.accounts));
    tracing::info!(
        accounts = insp.accounts,
        "setup wizard imported the existing data; reloading into normal mode"
    );
    complete(&ctx);
    Ok(Json(done))
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

fn clean(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Sets up a new server in a database without OurShell data.
async fn finish(
    State(ctx): State<SetupCtx>,
    Json(body): Json<FinishRequest>,
) -> AppResult<Json<DoneResponse>> {
    check_code(&ctx, &body.code)?;
    check_password(&body.admin_password)?;
    let url = pick_database_url(&ctx, body.database_url)?;
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
    let pool = connect(&url).await?;
    let insp = startup::inspect(&pool).await.map_err(internal)?;
    if !insp.kind.is_new() {
        return Err(error(
            StatusCode::CONFLICT,
            "existing_data",
            "this database already has an OurShell server's data: import it, or clear it first",
        ));
    }
    let key_b64 = ctx.known_key().unwrap_or_else(config::generate_master_key);
    let key = config::decode_master_key(&key_b64).map_err(|e| {
        error(
            StatusCode::BAD_REQUEST,
            "invalid_master_key",
            format!("the configured master key is invalid: {e}"),
        )
    })?;
    let hash =
        password::hash(&body.admin_password).map_err(|e| AppError::internal(e.to_string()))?;

    // Settings already in config.toml (hand-written) are the starting point;
    // the wizard's answers go on top, and all of it into the database.
    let file = settings::read(&ctx.data_dir).map_err(internal)?;
    let mut db = file.without_bootstrap();
    let locked = |k: &str| ctx.effective.env_locked.contains(&k);
    db.public_url = public_url;
    if !locked("behind_proxy") {
        db.behind_proxy = Some(body.behind_proxy);
    }
    let behind_proxy = ctx
        .effective
        .values
        .behind_proxy
        .filter(|_| locked("behind_proxy"))
        .unwrap_or(body.behind_proxy);
    db.tls = clean(body.tls_domain)
        .filter(|_| !behind_proxy)
        .map(|domain| FileTls {
            domain,
            email: clean(body.tls_email),
        });
    db.signups_enabled = Some(body.signups_enabled);
    db.invite_only = Some(body.invite_only);
    db.max_devices_per_user = body.max_devices_per_user;
    db.admin_password_hash = Some(hash);

    migrate(&pool).await?;
    crate::state::load_or_create_server_setup(&pool, &key)
        .await
        .map_err(|e| {
            error(
                StatusCode::CONFLICT,
                "existing_data",
                format!("{e:#}; clear the database first"),
            )
        })?;
    settings::db::write(&pool, &key, &db)
        .await
        .map_err(internal)?;
    pool.close().await;

    let shown = (!ctx.key_from_env()).then(|| key_b64.clone());
    let done = save_bootstrap(&ctx, &url, &key_b64, true, shown, None);
    tracing::info!("setup complete; reloading into normal mode");
    complete(&ctx);
    Ok(Json(done))
}
