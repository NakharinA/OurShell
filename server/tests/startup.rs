//! The startup decision (app or setup wizard), settings in the database,
//! and the wizard's inspect / import / clear for existing data. Each test
//! gets its own schema and an empty data dir; "restarting" the server is
//! running `startup::decide` again.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use ourshell_server::settings::{self, FileBackup, FileConfig};
use ourshell_server::startup::{self, DbKind, Decision, KeyStatus, Phase, Remembered};
use ourshell_server::state::AppState;
use ourshell_server::web::{admin, password, setup, Reload};
use serde_json::{json, Value};
use tower::ServiceExt;

const CODE: &str = "ABCD-EFGH-JKMN-PQRS";
const ADMIN_PW: &str = "a long admin password";

fn temp_dir() -> PathBuf {
    std::env::temp_dir().join(format!("ourshell-startup-{}", uuid::Uuid::new_v4()))
}

/// A fake environment.
fn env_of(pairs: Vec<(&'static str, String)>) -> impl Fn(&str) -> Option<String> {
    move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone())
}

async fn decide(dir: &Path, env: &dyn Fn(&str) -> Option<String>) -> Decision {
    startup::decide(dir, env, &Remembered::default(), Duration::ZERO)
        .await
        .unwrap()
}

fn setup_state(d: Decision) -> startup::SetupState {
    match d {
        Decision::Setup(s) => *s,
        Decision::Ready(_) => panic!("expected setup mode, got ready"),
    }
}

fn ready(d: Decision) -> startup::Ready {
    match d {
        Decision::Ready(r) => *r,
        Decision::Setup(s) => panic!("expected ready, got setup: {s:?}"),
    }
}

/// The setup wizard the server would serve for `state` with `env`.
fn wizard(dir: &Path, env: &dyn Fn(&str) -> Option<String>, state: startup::SetupState) -> Router {
    let effective = settings::resolve(&settings::read(dir).unwrap(), None, env).unwrap();
    setup::router(
        setup::SetupCtx::new(dir.to_path_buf(), CODE.into(), effective, Reload::default())
            .with_startup(state, Remembered::default()),
    )
}

async fn post(app: &Router, path: &str, body: Value) -> (StatusCode, Value) {
    call(app, "POST", path, None, body).await
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-ourshell-admin", "1");
    if let Some(c) = cookie {
        req = req.header(header::COOKIE, c);
    }
    let res = app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let cookie = res
        .headers()
        .get(header::SET_COOKIE)
        .map(|v| v.to_str().unwrap().split(';').next().unwrap().to_string());
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let mut v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if let (Some(c), Value::Object(m)) = (cookie, &mut v) {
        m.insert("_cookie".into(), json!(c));
    }
    (status, v)
}

fn finish_body(url: Option<&str>) -> Value {
    json!({
        "code": CODE,
        "database_url": url,
        "public_url": "https://sync.example.com",
        "signups_enabled": true,
        "invite_only": true,
        "max_devices_per_user": 3,
        "admin_password": ADMIN_PW,
    })
}

async fn user_count(db: &common::TempSchema) -> i64 {
    let pool = db.pool().await;
    let (n,): (i64,) = sqlx_core::query_as::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    pool.close().await;
    n
}

/// Signs up one account on the app behind `r`.
async fn sign_up(r: &startup::Ready, email: &str) {
    let mut config = r.config.clone();
    config.rate_limit_per_min = 1000;
    let state = AppState::new(r.pool.clone(), config).await.unwrap();
    let app = ourshell_server::routes::build_router(state);
    let (s, body) = common::register_user(&app, email, "pw", None, common::new_device("d")).await;
    assert_eq!(s, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn no_url_or_unreachable_database_means_setup() {
    let dir = temp_dir();
    let st = setup_state(decide(&dir, &|_| None).await);
    assert_eq!(st.phase, Phase::NoDatabase);

    // Nothing listens on port 1: unreachable, with the password redacted.
    let env = env_of(vec![(
        "DATABASE_URL",
        "postgres://ourshell:hunter2-secret@127.0.0.1:1/ourshell".into(),
    )]);
    let st = setup_state(decide(&dir, &env).await);
    assert_eq!(st.phase, Phase::Unreachable);
    assert!(st.database_from_env);
    assert_eq!(st.database.as_deref(), Some("127.0.0.1:1/ourshell"));
    let err = st.error.unwrap();
    assert!(!err.contains("hunter2"), "{err}");
    let shown = serde_json::to_string(&setup_state(decide(&dir, &env).await)).unwrap();
    assert!(!shown.contains("hunter2"), "{shown}");
    std::fs::remove_dir_all(dir).ok();
}

/// Env DATABASE_URL + SERVER_MASTER_KEY: the wizard runs once on a fresh
/// database; after that a restart with an empty data dir goes straight to
/// ready with the settings from the database, and /admin changes survive a
/// restart too.
#[tokio::test]
async fn env_only_deployment_needs_no_data_volume() {
    let Some(db) = common::temp_schema().await else {
        return;
    };
    let key = ourshell_server::config::generate_master_key();
    let env = env_of(vec![
        ("DATABASE_URL", db.url.clone()),
        ("SERVER_MASTER_KEY", key.clone()),
    ]);

    // A fresh database: the wizard, with the database step already done.
    let dir1 = temp_dir();
    let st = setup_state(decide(&dir1, &env).await);
    assert_eq!(st.phase, Phase::New);
    assert!(st.database_from_env && st.key_from_env);
    assert_eq!(st.inspection.as_ref().unwrap().kind, DbKind::Empty);

    let app = wizard(&dir1, &env, st);
    let (s, body) = post(&app, "/setup/api/inspect", json!({"code": CODE})).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["inspection"]["kind"], "empty", "{body}");
    let (s, body) = post(&app, "/setup/api/finish", finish_body(None)).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert!(
        body["master_key"].is_null(),
        "the key comes from the environment"
    );
    assert_eq!(body["env_lines"], json!([]));

    // "Redeploy": a new, empty data dir.
    let dir2 = temp_dir();
    let r = ready(decide(&dir2, &env).await);
    assert!(r.config.invite_only);
    assert_eq!(r.config.max_devices_per_user, 3);
    assert_eq!(
        r.config.public_url.as_deref(),
        Some("https://sync.example.com")
    );
    assert!(password::verify(
        ADMIN_PW,
        r.config.admin_password_hash.as_deref().unwrap()
    ));
    assert!(r.key_from_env);
    sign_up(&r, "owner@example.com").await;

    // An /admin change is saved in the database...
    let state = AppState::new(r.pool.clone(), r.config.clone())
        .await
        .unwrap();
    let web = admin::router(admin::AdminCtx {
        state,
        data_dir: dir2.clone(),
        sessions: Arc::new(admin::Sessions::default()),
        reload: Reload::default(),
        effective: Arc::new(r.effective.clone()),
        tls_status: Arc::new(Default::default()),
        backups: ourshell_server::backup::Backups::new(dir2.clone()),
    });
    let (s, body) = call(
        &web,
        "POST",
        "/admin/api/login",
        None,
        json!({"password": ADMIN_PW}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let cookie = body["_cookie"].as_str().unwrap().to_string();
    let (_, body) = call(
        &web,
        "GET",
        "/admin/api/settings",
        Some(&cookie),
        Value::Null,
    )
    .await;
    let mut form = body["settings"].clone();
    form["signups_enabled"] = json!(false);
    form["session_ttl_hours"] = json!(48);
    let (s, body) = call(&web, "PUT", "/admin/api/settings", Some(&cookie), form).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let (s, _) = call(
        &web,
        "PUT",
        "/admin/api/backup",
        Some(&cookie),
        json!({"enabled": false, "bucket": "kept-bucket", "interval_hours": 6, "keep": 2,
               "passphrase": "a backup passphrase"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    r.pool.close().await;

    // ...so another redeploy with yet another empty data dir keeps it.
    let dir3 = temp_dir();
    let r = ready(decide(&dir3, &env).await);
    assert!(!r.config.signups_enabled);
    assert_eq!(r.config.session_ttl_hours, 48);
    let b = r.effective.values.backup.clone().unwrap();
    assert_eq!(b.bucket.as_deref(), Some("kept-bucket"));
    assert_eq!(b.passphrase.as_deref(), Some("a backup passphrase"));
    // Nothing was written to the data dir, and the key never to the database.
    assert!(!dir3.join(settings::CONFIG_FILE).exists());
    let raw = settings::db::read_raw(&r.pool).await.unwrap().unwrap();
    assert!(!raw.windows(key.len()).any(|w| w == key.as_bytes()));
    assert!(!raw.windows(10).any(|w| w == b"kept-bucke"));
    r.pool.close().await;

    for d in [dir1, dir2, dir3] {
        std::fs::remove_dir_all(d).ok();
    }
    db.drop().await;
}

/// Existing data, but the master key is missing (lost data volume) or
/// wrong: the wizard's existing-data mode; import with the right key keeps
/// every account, a wrong key is refused.
#[tokio::test]
async fn existing_data_without_the_key_is_imported() {
    let Some(db) = common::temp_schema().await else {
        return;
    };
    let key = ourshell_server::config::generate_master_key();
    let full_env = env_of(vec![
        ("DATABASE_URL", db.url.clone()),
        ("SERVER_MASTER_KEY", key.clone()),
    ]);
    let dir = temp_dir();
    let app = wizard(&dir, &full_env, setup_state(decide(&dir, &full_env).await));
    let (s, body) = post(&app, "/setup/api/finish", finish_body(None)).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let r = ready(decide(&dir, &full_env).await);
    sign_up(&r, "owner@example.com").await;
    r.pool.close().await;

    // A wrong key in the environment: existing data, "doesn't match".
    let wrong = ourshell_server::config::generate_master_key();
    let wrong_env = env_of(vec![
        ("DATABASE_URL", db.url.clone()),
        ("SERVER_MASTER_KEY", wrong.clone()),
    ]);
    let st = setup_state(decide(&temp_dir(), &wrong_env).await);
    assert_eq!(
        (st.phase, st.key_status),
        (Phase::ExistingData, KeyStatus::Wrong)
    );

    // Only DATABASE_URL (the key lived in a lost config.toml).
    let url_env = env_of(vec![("DATABASE_URL", db.url.clone())]);
    let dir2 = temp_dir();
    let st = setup_state(decide(&dir2, &url_env).await);
    assert_eq!(
        (st.phase, st.key_status),
        (Phase::ExistingData, KeyStatus::Missing)
    );
    let insp = st.inspection.clone().unwrap();
    assert_eq!(insp.kind, DbKind::Existing);
    assert_eq!(insp.accounts, 1);
    assert!(insp.schema_version.is_some() && insp.first_org_created.is_some());

    let app = wizard(&dir2, &url_env, st);
    // Every call needs the setup code.
    let (s, _) = post(
        &app,
        "/setup/api/import",
        json!({"code": "WRONG", "master_key": key}),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, body) = post(&app, "/setup/api/inspect", json!({"code": CODE})).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["inspection"]["kind"], "existing");
    assert_eq!(body["inspection"]["accounts"], 1);
    assert_eq!(body["key_status"], "missing");
    // A new setup on top of existing data is refused.
    let (s, body) = post(&app, "/setup/api/finish", finish_body(None)).await;
    assert_eq!(
        (s, body["error"].as_str()),
        (StatusCode::CONFLICT, Some("existing_data"))
    );
    // Wrong and malformed keys are refused; nothing changes.
    let (s, body) = post(
        &app,
        "/setup/api/import",
        json!({"code": CODE, "master_key": wrong}),
    )
    .await;
    assert_eq!(
        (s, body["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("wrong_master_key"))
    );
    let (s, body) = post(
        &app,
        "/setup/api/import",
        json!({"code": CODE, "master_key": "nope"}),
    )
    .await;
    assert_eq!(
        (s, body["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid_master_key"))
    );
    assert!(!dir2.join(settings::CONFIG_FILE).exists());
    // The right key: imported.
    let (s, body) = post(
        &app,
        "/setup/api/import",
        json!({"code": CODE, "master_key": key}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["accounts"], 1);
    assert_eq!(body["persistent"], true);
    assert_eq!(
        settings::read(&dir2).unwrap().server_master_key.as_deref(),
        Some(key.as_str())
    );

    // Restart: ready, the account and the settings are intact.
    let r = ready(decide(&dir2, &url_env).await);
    assert!(r.config.invite_only);
    assert!(!r.key_from_env);
    r.pool.close().await;
    assert_eq!(user_count(&db).await, 1);

    for d in [dir, dir2] {
        std::fs::remove_dir_all(d).ok();
    }
    db.drop().await;
}

/// Import into a database whose settings have no admin password (an
/// env-only 0.8 server): the wizard asks for one.
#[tokio::test]
async fn import_asks_for_an_admin_password_when_there_is_none() {
    let Some(db) = common::temp_schema().await else {
        return;
    };
    // An env-only server's database: migrated, server_setup, no settings.
    let key_b64 = ourshell_server::config::generate_master_key();
    let key = ourshell_server::config::decode_master_key(&key_b64).unwrap();
    let pool = db.pool().await;
    ourshell_server::migrator()
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();
    ourshell_server::state::load_or_create_server_setup(&pool, &key)
        .await
        .unwrap();
    pool.close().await;

    let env = env_of(vec![("DATABASE_URL", db.url.clone())]);
    let dir = temp_dir();
    let st = setup_state(decide(&dir, &env).await);
    assert_eq!(st.phase, Phase::ExistingData);
    let app = wizard(&dir, &env, st);
    let (s, body) = post(
        &app,
        "/setup/api/import",
        json!({"code": CODE, "master_key": key_b64}),
    )
    .await;
    assert_eq!(
        (s, body["error"].as_str()),
        (StatusCode::CONFLICT, Some("admin_password_required"))
    );
    let (s, body) = post(
        &app,
        "/setup/api/import",
        json!({"code": CODE, "master_key": key_b64, "admin_password": ADMIN_PW}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let r = ready(decide(&dir, &env).await);
    assert!(password::verify(
        ADMIN_PW,
        r.config.admin_password_hash.as_deref().unwrap()
    ));
    r.pool.close().await;
    std::fs::remove_dir_all(dir).ok();
    db.drop().await;
}

#[tokio::test]
async fn clear_needs_the_confirmation_and_drops_only_ourshell_tables() {
    let Some(db) = common::temp_schema().await else {
        return;
    };
    let key = ourshell_server::config::generate_master_key();
    let env = env_of(vec![
        ("DATABASE_URL", db.url.clone()),
        ("SERVER_MASTER_KEY", key.clone()),
    ]);
    let dir = temp_dir();
    let app = wizard(&dir, &env, setup_state(decide(&dir, &env).await));
    let (s, body) = post(&app, "/setup/api/finish", finish_body(None)).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let pool = db.pool().await;
    sqlx_core::query::query("CREATE TABLE their_table (id int)")
        .execute(&pool)
        .await
        .unwrap();

    let other = env_of(vec![("DATABASE_URL", db.url.clone())]);
    let st = setup_state(decide(&dir.join("x"), &other).await);
    assert_eq!(
        st.inspection.as_ref().unwrap().other_tables,
        vec!["their_table"]
    );
    let app = wizard(&dir.join("x"), &other, st);
    for confirm in [json!(null), json!("delete"), json!("yes")] {
        let (s, body) = post(
            &app,
            "/setup/api/clear",
            json!({"code": CODE, "confirm": confirm}),
        )
        .await;
        assert_eq!(
            (s, body["error"].as_str()),
            (StatusCode::BAD_REQUEST, Some("confirmation_required")),
            "{confirm}"
        );
    }
    let (s, _) = post(
        &app,
        "/setup/api/clear",
        json!({"code": "WRONG", "confirm": "DELETE"}),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(
        startup::inspect(&pool).await.unwrap().kind,
        DbKind::Existing
    );

    let (s, body) = post(
        &app,
        "/setup/api/clear",
        json!({"code": CODE, "confirm": "DELETE"}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["inspection"]["kind"], "foreign");
    assert_eq!(
        body["dropped"].as_array().unwrap().len(),
        startup::OURSHELL_TABLES.len()
    );
    // Nothing of OurShell's is left: no tables, sequences or indexes.
    let (left,): (i64,) = sqlx_core::query_as::query_as(
        "SELECT COUNT(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = current_schema() AND c.relname <> 'their_table'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(left, 0);
    assert_eq!(
        startup::list_tables(&pool).await.unwrap(),
        vec!["their_table"]
    );
    sqlx_core::query::query("DROP TABLE their_table")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;

    // ... and it can be set up anew.
    let st = setup_state(decide(&dir, &env).await);
    assert_eq!(st.inspection.as_ref().unwrap().kind, DbKind::Empty);
    let app = wizard(&dir, &env, st);
    let (s, body) = post(&app, "/setup/api/finish", finish_body(None)).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    ready(decide(&dir, &env).await).pool.close().await;
    std::fs::remove_dir_all(dir).ok();
    db.drop().await;
}

/// A 0.8.x `config.toml` with every setting: on the first start they move
/// into the database, the file keeps only the bootstrap keys, and the
/// original is kept next to it.
#[tokio::test]
async fn legacy_config_toml_settings_move_into_the_database() {
    let Some(db) = common::temp_schema().await else {
        return;
    };
    let key_b64 = ourshell_server::config::generate_master_key();
    let key = ourshell_server::config::decode_master_key(&key_b64).unwrap();
    // What 0.8 left behind: a migrated database with server_setup...
    let pool = db.pool().await;
    let migrator = sqlx_core::migrate::Migrator::new(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("migrations")
            .as_path(),
    )
    .await
    .unwrap();
    migrator.run(&pool).await.unwrap();
    sqlx_core::query::query("DROP TABLE server_settings")
        .execute(&pool)
        .await
        .unwrap();
    sqlx_core::query::query("DELETE FROM _sqlx_migrations WHERE version = 3")
        .execute(&pool)
        .await
        .unwrap();
    ourshell_server::state::load_or_create_server_setup(&pool, &key)
        .await
        .unwrap();
    // ...and a config.toml with everything in it.
    let dir = temp_dir();
    let legacy = FileConfig {
        database_url: Some(db.url.clone()),
        server_master_key: Some(key_b64.clone()),
        invite_only: Some(true),
        max_devices_per_user: Some(7),
        admin_password_hash: Some(password::hash(ADMIN_PW).unwrap()),
        backup: Some(FileBackup {
            bucket: Some("legacy-bucket".into()),
            passphrase: Some("legacy passphrase".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    settings::write(&dir, &legacy).unwrap();
    let original = std::fs::read_to_string(dir.join(settings::CONFIG_FILE)).unwrap();

    let r = ready(decide(&dir, &|_| None).await);
    assert!(r.config.invite_only);
    assert_eq!(r.config.max_devices_per_user, 7);
    assert!(
        r.effective.file_locked.is_empty(),
        "{:?}",
        r.effective.file_locked
    );
    let stored = settings::db::read(&pool, &key).await.unwrap().unwrap();
    assert_eq!(stored, legacy.without_bootstrap());
    // config.toml: only the bootstrap keys now; the original is kept.
    let file = settings::read(&dir).unwrap();
    assert_eq!(file, legacy.bootstrap());
    let copy = dir.join(format!(
        "{}.{}",
        settings::CONFIG_FILE,
        settings::LEGACY_COPY_SUFFIX
    ));
    assert_eq!(std::fs::read_to_string(copy).unwrap(), original);
    r.pool.close().await;

    // Restarting changes nothing more; settings still come from the database.
    let r = ready(decide(&dir, &|_| None).await);
    assert!(r.config.invite_only);
    r.pool.close().await;

    // A key put back into config.toml by hand overrides the database and
    // shows read-only on /admin.
    let mut file = settings::read(&dir).unwrap();
    file.invite_only = Some(false);
    settings::write(&dir, &file).unwrap();
    let r = ready(decide(&dir, &|_| None).await);
    assert!(!r.config.invite_only);
    assert_eq!(r.effective.file_locked, vec!["invite_only"]);
    assert!(
        settings::db::read(&pool, &key)
            .await
            .unwrap()
            .unwrap()
            .invite_only
            == Some(true)
    );
    r.pool.close().await;

    pool.close().await;
    std::fs::remove_dir_all(dir).ok();
    db.drop().await;
}

#[tokio::test]
async fn set_admin_password_writes_the_database() {
    let Some(db) = common::temp_schema().await else {
        return;
    };
    let key = ourshell_server::config::generate_master_key();
    let env = env_of(vec![
        ("DATABASE_URL", db.url.clone()),
        ("SERVER_MASTER_KEY", key.clone()),
    ]);
    let dir = temp_dir();
    // A stale hash in config.toml would override the database: removed.
    settings::write(
        &dir,
        &FileConfig {
            admin_password_hash: Some(password::hash("old admin password").unwrap()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(ourshell_server::set_admin_password_in(&dir, "short", &env)
        .await
        .is_err());
    ourshell_server::set_admin_password_in(&dir, "the new admin password", &env)
        .await
        .unwrap();
    assert_eq!(settings::read(&dir).unwrap().admin_password_hash, None);
    // With settings in it, the database counts as set up: no wizard.
    let r = ready(decide(&temp_dir(), &env).await);
    assert!(password::verify(
        "the new admin password",
        r.config.admin_password_hash.as_deref().unwrap()
    ));
    r.pool.close().await;
    // A wrong key is refused.
    let wrong = env_of(vec![
        ("DATABASE_URL", db.url.clone()),
        (
            "SERVER_MASTER_KEY",
            ourshell_server::config::generate_master_key(),
        ),
    ]);
    let err = ourshell_server::set_admin_password_in(&dir, "the new admin password", &wrong)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("doesn't match"), "{err}");
    std::fs::remove_dir_all(dir).ok();
    db.drop().await;
}
