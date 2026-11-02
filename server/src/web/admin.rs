//! The `/admin` page, served once the server is set up. Guarded by the
//! admin password from the setup wizard (Argon2id hash in `config.toml`);
//! a successful login sets an HttpOnly, SameSite=Strict session cookie, and
//! every state-changing call must also send `X-OurShell-Admin: 1`, which a
//! cross-site form can't.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, FromRequestParts, Path, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::middleware;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post, put};
use axum::{Extension, Json, Router};
use ourshell_proto::sync::OrgRole;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::{password, Reload, VERSION};
use crate::crypto;
use crate::db;
use crate::error::{AppError, AppResult};
use crate::rate_limit::{limit_with, RateLimiter};
use crate::routes::admin as org_admin;
use crate::settings::{self, FileOidc, FileTls};
use crate::sqlx;
use crate::state::AppState;
use crate::tls::{TlsStatus, ViaTls};

mod backup;

const COOKIE: &str = "ourshell_admin";
const SESSION_TTL: Duration = Duration::from_secs(12 * 3600);
const CSRF_HEADER: &str = "x-ourshell-admin";

/// Admin page sessions, kept across reloads so saving settings doesn't sign
/// the admin out. Only token hashes are stored.
#[derive(Default)]
pub struct Sessions(Mutex<HashMap<Vec<u8>, Instant>>);

impl Sessions {
    fn create(&self) -> String {
        let token = crypto::random_token();
        let mut map = self.0.lock().unwrap();
        let now = Instant::now();
        map.retain(|_, exp| *exp > now);
        map.insert(crypto::hash_token(&token), now + SESSION_TTL);
        token
    }

    fn valid(&self, token: &str) -> bool {
        self.0
            .lock()
            .unwrap()
            .get(&crypto::hash_token(token))
            .is_some_and(|exp| *exp > Instant::now())
    }

    fn remove(&self, token: &str) {
        self.0.lock().unwrap().remove(&crypto::hash_token(token));
    }

    /// Signs out everyone (after a password change).
    fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
}

#[derive(Clone)]
pub struct AdminCtx {
    pub state: AppState,
    pub data_dir: PathBuf,
    pub sessions: Arc<Sessions>,
    pub reload: Reload,
    /// Settings keys pinned by environment variables (read-only here).
    pub env_locked: Arc<Vec<&'static str>>,
    /// State of the built-in HTTPS listener, for the status card.
    pub tls_status: Arc<TlsStatus>,
    /// Scheduled backups (shared across reloads), for the Backups tab.
    pub backups: Arc<crate::backup::Backups>,
}

pub fn router(ctx: AdminCtx) -> Router {
    let login =
        Router::new()
            .route("/login", post(login))
            .route_layer(middleware::from_fn_with_state(
                Arc::new(RateLimiter::new(10).with_proxy(ctx.state.config().proxy.clone())),
                limit_with,
            ));
    let api = Router::new()
        .merge(login)
        .route("/logout", post(logout))
        .route("/status", get(status))
        .route("/settings", get(get_settings).put(put_settings))
        .route("/password", put(change_password))
        .route("/users", get(users))
        .route("/users/{user_id}/disable", post(disable_user))
        .route("/users/{user_id}/enable", post(enable_user))
        .route("/invites", get(invites).post(create_invite))
        .merge(backup::routes());
    Router::new()
        .route("/", get(|| async { Redirect::to("/admin") }))
        .route("/setup", get(|| async { Redirect::to("/admin") }))
        .route("/admin", get(|| async { super::page(super::ADMIN_HTML) }))
        // Public: where an invite link (`/invite#code=...`) lands. The code
        // is in the fragment, so it's never sent here.
        .route("/invite", get(|| async { super::page(super::INVITE_HTML) }))
        .nest("/admin/api", api)
        .merge(super::assets())
        .with_state(ctx)
}

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|kv| {
            let (k, v) = kv.trim().split_once('=')?;
            (k == COOKIE).then(|| v.to_string())
        })
}

/// Proof of a signed-in admin, checked by every API handler but `login`.
pub struct Admin;

impl FromRequestParts<AdminCtx> for Admin {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, ctx: &AdminCtx) -> Result<Self, AppError> {
        if parts.method != Method::GET && parts.headers.get(CSRF_HEADER).is_none() {
            return Err(AppError::forbidden(
                "csrf",
                "missing X-OurShell-Admin header",
            ));
        }
        match cookie_token(&parts.headers) {
            Some(t) if ctx.sessions.valid(&t) => Ok(Admin),
            _ => Err(AppError::unauthorized("sign in to the admin page")),
        }
    }
}

fn session_cookie(value: &str, max_age: u64, secure: bool) -> String {
    format!(
        "{COOKIE}={value}; Path=/admin; HttpOnly; SameSite=Strict; Max-Age={max_age}{}",
        if secure { "; Secure" } else { "" }
    )
}

/// The TCP peer, when the server was reached over a real socket.
type Peer = Option<Extension<ConnectInfo<SocketAddr>>>;

/// Whether the browser reached us over HTTPS: on the built-in HTTPS
/// listener (which tags requests with [`ViaTls`]), or through a trusted
/// reverse proxy that says so (`X-Forwarded-Proto`, believed only in proxy
/// mode and only from a trusted peer). `Secure` cookies would be dropped on
/// plain HTTP, which keeps serving the LAN even when HTTPS is on, so a
/// configured TLS domain alone isn't enough.
fn is_https(
    ctx: &AdminCtx,
    headers: &HeaderMap,
    via_tls: Option<Extension<ViaTls>>,
    peer: Peer,
) -> bool {
    via_tls.is_some()
        || peer.is_some_and(|Extension(ConnectInfo(addr))| {
            ctx.state.config().proxy.forwarded_https(addr.ip(), headers)
        })
}

#[derive(Deserialize)]
struct LoginRequest {
    password: String,
}

async fn login(
    State(ctx): State<AdminCtx>,
    via_tls: Option<Extension<ViaTls>>,
    peer: Peer,
    headers: HeaderMap,
    Json(body): Json<LoginRequest>,
) -> AppResult<Response> {
    if headers.get(CSRF_HEADER).is_none() {
        return Err(AppError::forbidden(
            "csrf",
            "missing X-OurShell-Admin header",
        ));
    }
    let Some(hash) = ctx.state.config().admin_password_hash.clone() else {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "no_admin_password",
            "no admin password is set; run `ourshell-server set-admin-password` in the server container",
        ));
    };
    let pw = body.password;
    let ok = tokio::task::spawn_blocking(move || password::verify(&pw, &hash))
        .await
        .unwrap_or(false);
    if !ok {
        return Err(AppError::unauthorized("wrong password"));
    }
    let token = ctx.sessions.create();
    let cookie = session_cookie(
        &token,
        SESSION_TTL.as_secs(),
        is_https(&ctx, &headers, via_tls, peer),
    );
    Ok(([(header::SET_COOKIE, cookie)], Json(json!({ "ok": true }))).into_response())
}

async fn logout(
    State(ctx): State<AdminCtx>,
    _: Admin,
    via_tls: Option<Extension<ViaTls>>,
    peer: Peer,
    headers: HeaderMap,
) -> Response {
    if let Some(t) = cookie_token(&headers) {
        ctx.sessions.remove(&t);
    }
    let cookie = session_cookie("", 0, is_https(&ctx, &headers, via_tls, peer));
    ([(header::SET_COOKIE, cookie)], Json(json!({ "ok": true }))).into_response()
}

async fn count(pool: &sqlx::PgPool, sql: &'static str) -> sqlx::Result<i64> {
    let (n,): (i64,) = sqlx::query_as(sql).fetch_one(pool).await?;
    Ok(n)
}

async fn status(State(ctx): State<AdminCtx>, _: Admin) -> AppResult<Json<serde_json::Value>> {
    let pool = ctx.state.pool();
    let cfg = ctx.state.config();
    let (db_version,): (String,) = sqlx::query_as("SHOW server_version")
        .fetch_one(pool)
        .await?;
    Ok(Json(json!({
        "version": VERSION,
        "public_url": cfg.public_url,
        "database": super::redact_database_url(&cfg.database_url),
        "database_version": db_version,
        "tls_domain": cfg.tls.as_ref().map(|t| t.domain.clone()),
        "tls_status": ctx.tls_status.get(),
        "users": count(pool, "SELECT COUNT(*) FROM users").await?,
        "devices": count(pool, "SELECT COUNT(*) FROM devices WHERE NOT revoked").await?,
        "items": count(pool, "SELECT COUNT(*) FROM items WHERE NOT deleted").await?,
        "has_owner": db::total_user_count(pool).await? > 0,
    })))
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct SettingsView {
    public_url: Option<String>,
    signups_enabled: bool,
    invite_only: bool,
    max_devices_per_user: u32,
    rate_limit_per_min: u32,
    session_ttl_hours: u32,
    tls_domain: Option<String>,
    tls_email: Option<String>,
    /// A reverse proxy / ingress terminates HTTPS.
    behind_proxy: bool,
    /// Comma-separated IPs/CIDRs; empty = loopback and private ranges.
    trusted_proxies: Option<String>,
    oidc_issuer: Option<String>,
    oidc_client_id: Option<String>,
    oidc_redirect_url: Option<String>,
    /// Never returned; on save, `None` keeps the stored secret.
    #[serde(skip_serializing)]
    oidc_client_secret: Option<String>,
    oidc_secret_set: bool,
}

async fn get_settings(State(ctx): State<AdminCtx>, _: Admin) -> Json<serde_json::Value> {
    let c = ctx.state.config();
    let view = SettingsView {
        public_url: c.public_url.clone(),
        signups_enabled: c.signups_enabled,
        invite_only: c.invite_only,
        max_devices_per_user: c.max_devices_per_user,
        rate_limit_per_min: c.rate_limit_per_min,
        session_ttl_hours: c.session_ttl_hours as u32,
        tls_domain: c.tls.as_ref().map(|t| t.domain.clone()),
        tls_email: c.tls.as_ref().and_then(|t| t.email.clone()),
        behind_proxy: c.proxy.enabled(),
        trusted_proxies: settings::load(&ctx.data_dir)
            .ok()
            .and_then(|e| e.values.trusted_proxies)
            .map(|l| l.join(", ")),
        oidc_issuer: c.oidc.as_ref().map(|o| o.issuer.clone()),
        oidc_client_id: c.oidc.as_ref().map(|o| o.client_id.clone()),
        oidc_redirect_url: c.oidc.as_ref().map(|o| o.redirect_url.clone()),
        oidc_client_secret: None,
        oidc_secret_set: c.oidc.as_ref().is_some_and(|o| !o.client_secret.is_empty()),
    };
    Json(json!({ "settings": view, "env_locked": *ctx.env_locked }))
}

fn clean(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

async fn put_settings(
    State(ctx): State<AdminCtx>,
    _: Admin,
    Json(new): Json<SettingsView>,
) -> AppResult<Json<serde_json::Value>> {
    let locked = |k: &str| ctx.env_locked.contains(&k);
    let mut file = settings::read(&ctx.data_dir).map_err(|e| AppError::internal(e.to_string()))?;
    let public_url = clean(new.public_url);
    if public_url
        .as_deref()
        .is_some_and(|u| !(u.starts_with("http://") || u.starts_with("https://")))
    {
        return Err(AppError::bad_request(
            "invalid_public_url",
            "the server address must start with http:// or https://",
        ));
    }
    if new.session_ttl_hours == 0 {
        return Err(AppError::bad_request(
            "invalid_ttl",
            "session lifetime must be at least 1 hour",
        ));
    }
    // Keys set by environment variables can't be changed here; leave them.
    if !locked("public_url") {
        file.public_url = public_url;
    }
    if !locked("signups_enabled") {
        file.signups_enabled = Some(new.signups_enabled);
    }
    if !locked("invite_only") {
        file.invite_only = Some(new.invite_only);
    }
    if !locked("max_devices_per_user") {
        file.max_devices_per_user = Some(new.max_devices_per_user);
    }
    if !locked("rate_limit_per_min") {
        file.rate_limit_per_min = Some(new.rate_limit_per_min);
    }
    if !locked("session_ttl_hours") {
        file.session_ttl_hours = Some(new.session_ttl_hours);
    }
    if !locked("trusted_proxies") {
        let list = clean(new.trusted_proxies)
            .map(|s| crate::proxy::split_list(&s))
            .filter(|l| !l.is_empty());
        if let Some(l) = &list {
            crate::proxy::ProxyTrust::behind_proxy(l)
                .map_err(|e| AppError::bad_request("invalid_trusted_proxies", e.to_string()))?;
        }
        file.trusted_proxies = list;
    }
    if !locked("behind_proxy") {
        file.behind_proxy = Some(new.behind_proxy);
    }
    let behind_proxy = if locked("behind_proxy") {
        ctx.state.config().proxy.enabled()
    } else {
        new.behind_proxy
    };
    if !locked("tls_domain") && !locked("tls_email") {
        // Behind a proxy the proxy owns HTTPS; the built-in certificate is off.
        file.tls = clean(new.tls_domain)
            .filter(|_| !behind_proxy)
            .map(|domain| FileTls {
                domain,
                email: clean(new.tls_email),
            });
    }
    if !locked("oidc") {
        file.oidc = match clean(new.oidc_issuer) {
            None => None,
            Some(issuer) => {
                let secret = clean(new.oidc_client_secret)
                    .or_else(|| file.oidc.as_ref().map(|o| o.client_secret.clone()))
                    .unwrap_or_default();
                Some(FileOidc {
                    issuer,
                    client_id: clean(new.oidc_client_id).unwrap_or_default(),
                    client_secret: secret,
                    redirect_url: clean(new.oidc_redirect_url).unwrap_or_default(),
                })
            }
        };
    }
    settings::write(&ctx.data_dir, &file).map_err(|e| AppError::internal(format!("{e:#}")))?;
    tracing::info!("admin page changed settings; reloading");
    ctx.reload.trigger_soon();
    Ok(Json(json!({ "ok": true, "reloading": true })))
}

#[derive(Deserialize)]
struct PasswordChange {
    current: String,
    new: String,
}

async fn change_password(
    State(ctx): State<AdminCtx>,
    _: Admin,
    Json(body): Json<PasswordChange>,
) -> AppResult<Json<serde_json::Value>> {
    let hash = ctx
        .state
        .config()
        .admin_password_hash
        .clone()
        .unwrap_or_default();
    let current = body.current;
    let ok = tokio::task::spawn_blocking(move || password::verify(&current, &hash))
        .await
        .unwrap_or(false);
    if !ok {
        return Err(AppError::unauthorized("the current password is wrong"));
    }
    if body.new.chars().count() < password::MIN_LEN {
        return Err(AppError::bad_request(
            "weak_password",
            format!(
                "the new password needs at least {} characters",
                password::MIN_LEN
            ),
        ));
    }
    let new_hash = password::hash(&body.new).map_err(|e| AppError::internal(e.to_string()))?;
    let mut file = settings::read(&ctx.data_dir).map_err(|e| AppError::internal(e.to_string()))?;
    file.admin_password_hash = Some(new_hash);
    settings::write(&ctx.data_dir, &file).map_err(|e| AppError::internal(format!("{e:#}")))?;
    ctx.sessions.clear();
    ctx.reload.trigger_soon();
    Ok(Json(json!({ "ok": true })))
}

async fn org(ctx: &AdminCtx) -> AppResult<Option<Uuid>> {
    Ok(db::default_org_id(ctx.state.pool()).await?)
}

async fn users(State(ctx): State<AdminCtx>, _: Admin) -> AppResult<Json<serde_json::Value>> {
    let users = match org(&ctx).await? {
        Some(org_id) => org_admin::org_users(ctx.state.pool(), org_id).await?,
        None => Vec::new(),
    };
    Ok(Json(json!({ "users": users })))
}

async fn set_disabled(ctx: AdminCtx, user_id: &str, disabled: bool) -> AppResult<StatusCode> {
    let user_id = db::parse_uuid(user_id, "user_id")?;
    let org_id = org(&ctx)
        .await?
        .ok_or_else(|| AppError::not_found("user_not_found", "no such user"))?;
    org_admin::set_user_disabled(ctx.state.pool(), org_id, user_id, disabled, None).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn disable_user(
    State(ctx): State<AdminCtx>,
    _: Admin,
    Path(user_id): Path<String>,
) -> AppResult<StatusCode> {
    set_disabled(ctx, &user_id, true).await
}

async fn enable_user(
    State(ctx): State<AdminCtx>,
    _: Admin,
    Path(user_id): Path<String>,
) -> AppResult<StatusCode> {
    set_disabled(ctx, &user_id, false).await
}

async fn invites(State(ctx): State<AdminCtx>, _: Admin) -> AppResult<Json<serde_json::Value>> {
    let Some(org_id) = org(&ctx).await? else {
        return Ok(Json(json!({ "invites": [] })));
    };
    let rows: Vec<(
        String,
        String,
        chrono::DateTime<chrono::Utc>,
        Option<String>,
    )> = sqlx::query_as(
        "SELECT i.code, i.role, i.created_at, u.email FROM invites i \
         LEFT JOIN users u ON u.id = i.used_by WHERE i.org_id = $1 ORDER BY i.created_at DESC",
    )
    .bind(org_id)
    .fetch_all(ctx.state.pool())
    .await?;
    let invites: Vec<_> = rows
        .into_iter()
        .map(|(code, role, created_at, used_by)| {
            json!({ "code": code, "role": role, "created_at": created_at, "used_by": used_by })
        })
        .collect();
    Ok(Json(json!({ "invites": invites })))
}

#[derive(Deserialize)]
struct NewInvite {
    role: Option<OrgRole>,
}

async fn create_invite(
    State(ctx): State<AdminCtx>,
    _: Admin,
    Json(body): Json<NewInvite>,
) -> AppResult<Json<serde_json::Value>> {
    let org_id = org(&ctx).await?.ok_or_else(|| {
        AppError::new(
            StatusCode::CONFLICT,
            "no_owner_yet",
            "sign up the first account from the OurShell app first (it becomes the owner); invites work after that",
        )
    })?;
    let role = match body.role {
        Some(OrgRole::Owner) => {
            return Err(AppError::bad_request(
                "invalid_role",
                "invites can't grant owner",
            ))
        }
        Some(r) => org_admin::role_to_db(&r),
        None => "member",
    };
    let code = org_admin::create_invite_in(ctx.state.pool(), org_id, role, None).await?;
    Ok(Json(json!({ "code": code })))
}
