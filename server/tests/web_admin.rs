//! The built-in web pages: the first-run setup wizard and the admin page.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use ourshell_server::config::Config;
use ourshell_server::settings::{self, Effective};
use ourshell_server::web::{admin, password, setup, Reload};
use serde_json::{json, Value};
use tower::ServiceExt;

const CODE: &str = "ABCD-EFGH-JKMN-PQRS";

fn temp_dir() -> PathBuf {
    std::env::temp_dir().join(format!("ourshell-web-{}", uuid::Uuid::new_v4()))
}

/// One request with optional cookie and the admin CSRF header; returns the
/// status, JSON body and any Set-Cookie value.
async fn call(
    app: &Router,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    csrf: bool,
    body: Option<Value>,
) -> (StatusCode, Value, Option<String>) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(c) = cookie {
        req = req.header(header::COOKIE, c);
    }
    if csrf {
        req = req.header("x-ourshell-admin", "1");
    }
    let body = body.map_or(Body::empty(), |b| Body::from(b.to_string()));
    let res = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let status = res.status();
    let set_cookie = res
        .headers()
        .get(header::SET_COOKIE)
        .map(|v| v.to_str().unwrap().to_string());
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value, set_cookie)
}

async fn reloaded(rx: &mut tokio::sync::watch::Receiver<u64>) -> bool {
    tokio::time::timeout(Duration::from_secs(5), rx.changed())
        .await
        .is_ok()
}

#[tokio::test]
async fn setup_wizard_writes_config_and_reloads() {
    // Only needs a reachable Postgres to test the connection against.
    if common::setup().await.is_none() {
        return;
    }
    let db_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| common::DEFAULT_TEST_DATABASE_URL.to_string());
    let dir = temp_dir();
    let reload = Reload::default();
    let mut rx = reload.subscribe();
    let app = setup::router(setup::SetupCtx::new(
        dir.clone(),
        CODE.into(),
        Effective::default(),
        reload,
    ));

    let (s, body, _) = call(&app, "GET", "/v1/health", None, false, None).await;
    assert_eq!((s, body["mode"].as_str()), (StatusCode::OK, Some("setup")));
    // The sync API explains itself instead of 404ing.
    let (s, body, _) = call(&app, "GET", "/v1/devices", None, false, None).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"], "not_set_up");

    let (s, _, _) = call(
        &app,
        "POST",
        "/setup/api/verify",
        None,
        false,
        Some(json!({"code": "WRONG"})),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // Case and dashes don't matter.
    let (s, _, _) = call(
        &app,
        "POST",
        "/setup/api/verify",
        None,
        false,
        Some(json!({"code": "abcdefghjkmnpqrs"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let (s, body, _) = call(
        &app,
        "POST",
        "/setup/api/test-db",
        None,
        false,
        Some(json!({"code": CODE, "database_url": db_url})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["ok"], true, "{body}");

    let mut finish = json!({
        "code": CODE,
        "database_url": db_url,
        "public_url": "https://sync.example.com",
        "signups_enabled": true,
        "invite_only": true,
        "max_devices_per_user": 4,
        "admin_password": "short",
    });
    let (s, body, _) = call(
        &app,
        "POST",
        "/setup/api/finish",
        None,
        false,
        Some(finish.clone()),
    )
    .await;
    assert_eq!(
        (s, body["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("weak_password"))
    );

    finish["admin_password"] = json!("a long admin password");
    let (s, body, _) = call(
        &app,
        "POST",
        "/setup/api/finish",
        None,
        false,
        Some(finish.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let master_key = body["master_key"].as_str().unwrap().to_string();
    assert!(body["config_toml"].as_str().unwrap().contains(&master_key));
    assert!(
        reloaded(&mut rx).await,
        "finishing setup reloads the server"
    );

    // The saved file is a complete config.
    let cfg = Config::load(&dir)
        .unwrap()
        .expect("config is complete after setup");
    assert_eq!(cfg.database_url, db_url);
    assert!(cfg.invite_only && cfg.signups_enabled);
    assert_eq!(cfg.max_devices_per_user, 4);
    assert_eq!(cfg.public_url.as_deref(), Some("https://sync.example.com"));
    assert!(password::verify(
        "a long admin password",
        cfg.admin_password_hash.as_deref().unwrap()
    ));

    // Setup is one-shot.
    let (s, _, _) = call(&app, "POST", "/setup/api/finish", None, false, Some(finish)).await;
    assert_eq!(s, StatusCode::CONFLICT);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn setup_keeps_installer_provided_database() {
    let dir = temp_dir();
    let effective = settings::apply_env(&Default::default(), &|k| {
        (k == "DATABASE_URL").then(|| "postgres://u:secret@db:5432/ourshell".to_string())
    })
    .unwrap();
    let app = setup::router(setup::SetupCtx::new(
        dir,
        CODE.into(),
        effective,
        Reload::default(),
    ));
    let (s, body, _) = call(&app, "GET", "/setup/api/info", None, false, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["database_from_env"], true);
    // Shown without credentials.
    assert_eq!(body["database"], "db:5432/ourshell");
}

#[tokio::test]
async fn admin_page() {
    let hash = password::hash("admin password 1").unwrap();
    let Some(app) = common::setup_with(|c| {
        c.admin_password_hash = Some(hash);
        c.invite_only = true;
    })
    .await
    else {
        return;
    };
    let dir = temp_dir();
    let reload = Reload::default();
    let mut rx = reload.subscribe();
    let web = admin::router(admin::AdminCtx {
        state: app.state.clone(),
        data_dir: dir.clone(),
        sessions: Arc::new(admin::Sessions::default()),
        reload,
        env_locked: Arc::new(vec!["rate_limit_per_min"]),
        tls_status: Arc::new(Default::default()),
    });

    // Signed out, and the CSRF header is required.
    let (s, _, _) = call(&web, "GET", "/admin/api/status", None, false, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let login = Some(json!({"password": "admin password 1"}));
    let (s, _, _) = call(&web, "POST", "/admin/api/login", None, false, login.clone()).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = call(
        &web,
        "POST",
        "/admin/api/login",
        None,
        true,
        Some(json!({"password": "nope"})),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // Over the built-in HTTPS listener the cookie is `Secure`...
    let via_tls = web
        .clone()
        .layer(axum::Extension(ourshell_server::tls::ViaTls));
    let (s, _, tls_cookie) = call(
        &via_tls,
        "POST",
        "/admin/api/login",
        None,
        true,
        login.clone(),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(tls_cookie.unwrap().contains("; Secure"));
    // ...but not over plain HTTP.
    let (s, _, set_cookie) = call(&web, "POST", "/admin/api/login", None, true, login).await;
    assert_eq!(s, StatusCode::OK);
    let set_cookie = set_cookie.unwrap();
    assert!(set_cookie.contains("HttpOnly") && set_cookie.contains("SameSite=Strict"));
    assert!(!set_cookie.contains("Secure"));
    let cookie = set_cookie.split(';').next().unwrap().to_string();
    let c = Some(cookie.as_str());

    let (s, status, _) = call(&web, "GET", "/admin/api/status", c, false, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        (status["users"].as_i64(), status["has_owner"].as_bool()),
        (Some(0), Some(false))
    );
    assert!(status["tls_status"].is_null());

    // Invites need an org, which the first sign-up creates.
    let (s, body, _) = call(
        &web,
        "POST",
        "/admin/api/invites",
        c,
        true,
        Some(json!({"role": "Member"})),
    )
    .await;
    assert_eq!(
        (s, body["error"].as_str()),
        (StatusCode::CONFLICT, Some("no_owner_yet"))
    );
    let (s, _) = common::register_user(
        &app.router,
        "owner@example.com",
        "pw-owner",
        None,
        common::new_device("a"),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, body, _) = call(
        &web,
        "POST",
        "/admin/api/invites",
        c,
        true,
        Some(json!({"role": "Owner"})),
    )
    .await;
    assert_eq!(
        (s, body["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid_role"))
    );
    let (s, body, _) = call(
        &web,
        "POST",
        "/admin/api/invites",
        c,
        true,
        Some(json!({"role": "Member"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let code = body["code"].as_str().unwrap().to_string();

    // A web-created invite works for an invite-only sign-up.
    let (s, _) = common::register_user(
        &app.router,
        "bob@example.com",
        "pw-bob",
        Some(&code),
        common::new_device("b"),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, body, _) = call(&web, "GET", "/admin/api/invites", c, false, None).await;
    assert_eq!(body["invites"][0]["used_by"], "bob@example.com");

    let (_, body, _) = call(&web, "GET", "/admin/api/users", c, false, None).await;
    let users = body["users"].as_array().unwrap();
    assert_eq!(users.len(), 2);
    let bob = users
        .iter()
        .find(|u| u["email"] == "bob@example.com")
        .unwrap();
    let bob_id = bob["user_id"].as_str().unwrap();
    let (s, _, _) = call(
        &web,
        "POST",
        &format!("/admin/api/users/{bob_id}/disable"),
        c,
        true,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, body, _) = call(&web, "GET", "/admin/api/users", c, false, None).await;
    assert!(body["users"]
        .as_array()
        .unwrap()
        .iter()
        .any(|u| u["email"] == "bob@example.com" && u["disabled"] == true));
    let (s, _, _) = call(
        &web,
        "POST",
        &format!("/admin/api/users/{bob_id}/enable"),
        c,
        true,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    // Settings: env-pinned keys are reported and left alone; others saved.
    let (_, body, _) = call(&web, "GET", "/admin/api/settings", c, false, None).await;
    assert_eq!(body["env_locked"], json!(["rate_limit_per_min"]));
    let mut new = body["settings"].clone();
    new["public_url"] = json!("ftp://nope");
    let (s, _, _) = call(
        &web,
        "PUT",
        "/admin/api/settings",
        c,
        true,
        Some(new.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    new["public_url"] = json!("https://sync.example.com");
    new["signups_enabled"] = json!(false);
    new["rate_limit_per_min"] = json!(5);
    let (s, _, _) = call(
        &web,
        "PUT",
        "/admin/api/settings",
        c,
        false,
        Some(new.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "PUT needs the CSRF header");
    let (s, _, _) = call(&web, "PUT", "/admin/api/settings", c, true, Some(new)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(reloaded(&mut rx).await);
    let file = settings::read(&dir).unwrap();
    assert_eq!(file.signups_enabled, Some(false));
    assert_eq!(file.public_url.as_deref(), Some("https://sync.example.com"));
    assert_eq!(file.rate_limit_per_min, None, "env-locked key not written");

    // Changing the password signs everyone out.
    let (s, _, _) = call(
        &web,
        "PUT",
        "/admin/api/password",
        c,
        true,
        Some(json!({"current": "wrong", "new": "another long password"})),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _, _) = call(
        &web,
        "PUT",
        "/admin/api/password",
        c,
        true,
        Some(json!({"current": "admin password 1", "new": "another long password"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, _) = call(&web, "GET", "/admin/api/status", c, false, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert!(password::verify(
        "another long password",
        settings::read(&dir)
            .unwrap()
            .admin_password_hash
            .as_deref()
            .unwrap()
    ));

    // Pages are served with a strict CSP.
    let res = web
        .clone()
        .oneshot(Request::get("/admin").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let csp = res.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap();
    assert!(csp.contains("script-src 'self'") && !csp.contains("unsafe-inline"));
    std::fs::remove_dir_all(dir).ok();
}

/// Logs in to the admin page as if over a real socket from `peer`, with
/// extra headers; returns the status and Set-Cookie.
async fn login_from(
    web: &Router,
    peer: &str,
    headers: &[(&str, &str)],
    password: &str,
) -> (StatusCode, String) {
    let mut req = Request::post("/admin/api/login")
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-ourshell-admin", "1")
        .extension(axum::extract::ConnectInfo(
            peer.parse::<std::net::SocketAddr>().unwrap(),
        ));
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let body = Body::from(json!({ "password": password }).to_string());
    let res = web.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let cookie = res
        .headers()
        .get(header::SET_COOKIE)
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    (res.status(), cookie)
}

fn admin_router(
    app: &common::TestApp,
    dir: &std::path::Path,
    env_locked: Vec<&'static str>,
) -> Router {
    admin::router(admin::AdminCtx {
        state: app.state.clone(),
        data_dir: dir.to_path_buf(),
        sessions: Arc::new(admin::Sessions::default()),
        reload: Reload::default(),
        env_locked: Arc::new(env_locked),
        tls_status: Arc::new(Default::default()),
    })
}

#[tokio::test]
async fn behind_proxy() {
    use ourshell_server::proxy::ProxyTrust;
    let https = [("x-forwarded-proto", "https")];

    // Proxy mode off: forwarding headers are ignored, even from loopback.
    let hash = password::hash("admin password 1").unwrap();
    let Some(plain) = common::setup_with(|c| c.admin_password_hash = Some(hash.clone())).await
    else {
        return;
    };
    let dir = temp_dir();
    let web = admin_router(&plain, &dir, vec![]);
    let (s, cookie) = login_from(&web, "127.0.0.1:5000", &https, "admin password 1").await;
    assert_eq!(s, StatusCode::OK);
    assert!(!cookie.contains("Secure"), "{cookie}");

    // Proxy mode on, default trusted ranges.
    let app = common::setup_with(|c| {
        c.admin_password_hash = Some(hash.clone());
        c.proxy = ProxyTrust::behind_proxy(&[]).unwrap();
    })
    .await
    .unwrap();
    let web = admin_router(&app, &dir, vec![]);
    // HTTPS forwarded by a trusted proxy: Secure cookie.
    let (s, cookie) = login_from(&web, "172.18.0.1:5000", &https, "admin password 1").await;
    assert_eq!(s, StatusCode::OK);
    assert!(cookie.contains("; Secure"), "{cookie}");
    // The same header straight from the internet is not believed.
    let (s, cookie) = login_from(&web, "198.51.100.9:5000", &https, "admin password 1").await;
    assert_eq!(s, StatusCode::OK);
    assert!(!cookie.contains("Secure"), "{cookie}");

    // The login rate limit (10/min) counts each forwarded client, not the
    // proxy: one client is throttled, another behind the same proxy isn't.
    for _ in 0..10 {
        let (s, _) = login_from(
            &web,
            "127.0.0.1:5000",
            &[("x-forwarded-for", "203.0.113.1")],
            "wrong",
        )
        .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }
    let (s, _) = login_from(
        &web,
        "127.0.0.1:5000",
        &[("x-forwarded-for", "203.0.113.1")],
        "wrong",
    )
    .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    let (s, _) = login_from(
        &web,
        "127.0.0.1:5000",
        &[("x-forwarded-for", "203.0.113.2")],
        "wrong",
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // Settings: proxy mode and trusted proxies are saved and validated, and
    // turning proxy mode on drops the built-in HTTPS domain.
    let (s, cookie) = login_from(
        &web,
        "127.0.0.1:5000",
        &[("x-forwarded-for", "203.0.113.3")],
        "admin password 1",
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let c = cookie.split(';').next().unwrap().to_string();
    let (_, body, _) = call(&web, "GET", "/admin/api/settings", Some(&c), false, None).await;
    assert_eq!(body["settings"]["behind_proxy"], true);
    let mut new = body["settings"].clone();
    new["trusted_proxies"] = json!("not-an-ip");
    let (s, body) = {
        let (s, b, _) = call(
            &web,
            "PUT",
            "/admin/api/settings",
            Some(&c),
            true,
            Some(new.clone()),
        )
        .await;
        (s, b)
    };
    assert_eq!(
        (s, body["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid_trusted_proxies"))
    );
    new["trusted_proxies"] = json!("10.42.0.0/16, 203.0.113.5");
    new["tls_domain"] = json!("sync.example.com");
    let (s, _, _) = call(
        &web,
        "PUT",
        "/admin/api/settings",
        Some(&c),
        true,
        Some(new),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let file = settings::read(&dir).unwrap();
    assert_eq!(file.behind_proxy, Some(true));
    assert_eq!(
        file.trusted_proxies,
        Some(vec!["10.42.0.0/16".to_string(), "203.0.113.5".to_string()])
    );
    assert_eq!(file.tls, None, "the proxy owns HTTPS");
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn setup_wizard_behind_proxy() {
    if common::setup().await.is_none() {
        return;
    }
    let db_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| common::DEFAULT_TEST_DATABASE_URL.to_string());
    let dir = temp_dir();
    let app = setup::router(setup::SetupCtx::new(
        dir.clone(),
        CODE.into(),
        Effective::default(),
        Reload::default(),
    ));
    let (s, body, _) = call(
        &app,
        "POST",
        "/setup/api/finish",
        None,
        false,
        Some(json!({
            "code": CODE,
            "database_url": db_url,
            "public_url": "https://sync.example.com",
            "tls_domain": "sync.example.com",
            "behind_proxy": true,
            "signups_enabled": true,
            "invite_only": true,
            "admin_password": "a long admin password",
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let file = settings::read(&dir).unwrap();
    assert_eq!(file.behind_proxy, Some(true));
    assert_eq!(
        file.tls, None,
        "behind a proxy, the built-in certificate is off"
    );
    let cfg = Config::load(&dir).unwrap().unwrap();
    assert!(cfg.proxy.enabled() && cfg.tls.is_none());
    std::fs::remove_dir_all(dir).unwrap();
}
