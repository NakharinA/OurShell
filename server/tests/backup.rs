//! Encrypted backups to S3-compatible storage, end to end: Postgres plus a
//! local MinIO. Skips (passes trivially) when either isn't reachable, e.g.
//!
//! ```sh
//! docker run -d --name osbk-minio -p 18560:9000 -e MINIO_ROOT_USER=osbk \
//!   -e MINIO_ROOT_PASSWORD=osbk-secret-123 minio/minio server /data
//! ```
//!
//! Override with `TEST_S3_ENDPOINT` / `TEST_S3_ACCESS_KEY` / `TEST_S3_SECRET_KEY`.

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use base64::Engine;
use common::{as_session, login_user, new_device, register_user, request};
use http_body_util::BodyExt;
use ourshell_proto::sync::{PullResponse, PushRequest, SyncItem};
use ourshell_server::backup::format::{self, KdfParams};
use ourshell_server::backup::s3::{S3Client, S3Config};
use ourshell_server::backup::{restore, Backups, Outcome, Trigger};
use ourshell_server::config::Config;
use ourshell_server::settings::{self, FileBackup, FileConfig};
use ourshell_server::web::{admin, password, Reload};
use serde_json::{json, Value};
use sqlx_core::executor::Executor;
use sqlx_core::sql_str::AssertSqlSafe;
use sqlx_postgres::{PgPool, PgPoolOptions};
use tower::ServiceExt;

const PASSPHRASE: &str = "backup passphrase 123";
const FAST: KdfParams = KdfParams {
    m_cost_kib: 64,
    t_cost: 1,
    p_cost: 1,
};

fn s3_env() -> (String, String, String) {
    let get = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
    (
        get("TEST_S3_ENDPOINT", "http://127.0.0.1:18560"),
        get("TEST_S3_ACCESS_KEY", "osbk"),
        get("TEST_S3_SECRET_KEY", "osbk-secret-123"),
    )
}

/// A fresh bucket on the test MinIO, or `None` (skip) if it isn't running.
async fn minio_bucket() -> Option<S3Config> {
    let (endpoint, ak, sk) = s3_env();
    let cfg = S3Config {
        endpoint: endpoint.clone(),
        region: "us-east-1".into(),
        bucket: format!("osbk-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]),
        access_key_id: ak,
        secret_access_key: sk,
        path_style: true,
    };
    let client = S3Client::new(cfg.clone()).unwrap();
    match tokio::time::timeout(Duration::from_secs(5), client.create_bucket()).await {
        Ok(Ok(())) => Some(cfg),
        Ok(Err(e)) => {
            println!("skipping: no usable S3 test endpoint at {endpoint} ({e:#})");
            None
        }
        Err(_) => {
            println!("skipping: S3 test endpoint at {endpoint} timed out");
            None
        }
    }
}

fn file_backup(s3: &S3Config, keep: u32) -> FileBackup {
    FileBackup {
        enabled: Some(true),
        endpoint: Some(s3.endpoint.clone()),
        region: None,
        bucket: Some(s3.bucket.clone()),
        prefix: Some("srv1".into()),
        access_key_id: Some(s3.access_key_id.clone()),
        secret_access_key: Some(s3.secret_access_key.clone()),
        path_style: Some(true),
        passphrase: Some(PASSPHRASE.into()),
        interval_hours: Some(24),
        keep: Some(keep),
    }
}

fn temp_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("ourshell-backup-{}", uuid::Uuid::new_v4()))
}

fn base_url() -> String {
    std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| common::DEFAULT_TEST_DATABASE_URL.to_string())
}

/// A pool on a brand-new, empty schema (no migrations yet).
async fn empty_schema() -> (PgPool, String) {
    let schema = format!("test_restore_{}", uuid::Uuid::new_v4().simple());
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&base_url())
        .await
        .unwrap();
    admin
        .execute(AssertSqlSafe(format!("CREATE SCHEMA \"{schema}\"")))
        .await
        .unwrap();
    admin.close().await;
    let s = schema.clone();
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .after_connect(move |conn, _| {
            let s = s.clone();
            Box::pin(async move {
                conn.execute(AssertSqlSafe(format!("SET search_path = \"{s}\"")))
                    .await?;
                Ok(())
            })
        })
        .connect(&base_url())
        .await
        .unwrap();
    (pool, schema)
}

async fn drop_schemas(schemas: &[String]) {
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&base_url())
        .await
        .unwrap();
    for s in schemas {
        admin
            .execute(AssertSqlSafe(format!(
                "DROP SCHEMA IF EXISTS \"{s}\" CASCADE"
            )))
            .await
            .unwrap();
    }
    admin.close().await;
}

async fn current_schema(pool: &PgPool) -> String {
    let (s,): (String,) = sqlx_core::query_as::query_as("SELECT current_schema()::text")
        .fetch_one(pool)
        .await
        .unwrap();
    s
}

async fn count(pool: &PgPool, table: &str) -> i64 {
    let (n,): (i64,) =
        sqlx_core::query_as::query_as(AssertSqlSafe(format!("SELECT COUNT(*) FROM \"{table}\"")))
            .fetch_one(pool)
            .await
            .unwrap();
    n
}

async fn push(app: &Router, token: &str, vault_id: &str, envelope: &str) {
    let (s, body) = request(
        app,
        "POST",
        &format!("/v1/vaults/{vault_id}/items"),
        Some(token),
        Some(&PushRequest {
            items: vec![SyncItem {
                id: uuid::Uuid::new_v4().to_string(),
                kind: "host".into(),
                version: 1,
                vault_id: vault_id.to_string(),
                deleted: false,
                envelope: envelope.into(),
                updated_at: None,
            }],
        }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
}

fn backup_keys(objects: &[ourshell_server::backup::s3::Object]) -> Vec<String> {
    let mut keys: Vec<String> = objects.iter().map(|o| o.key.clone()).collect();
    keys.sort();
    keys
}

#[tokio::test]
async fn back_up_dedup_retention_and_restore() {
    let Some(app) = common::setup().await else {
        return;
    };
    let Some(s3) = minio_bucket().await else {
        return;
    };
    let client = S3Client::new(s3.clone()).unwrap();
    let source_schema = current_schema(&app.pool).await;

    // A server with a user and a synced item.
    let device = new_device("d1");
    let (s, body) = register_user(
        &app.router,
        "owner@example.com",
        "owner pw",
        None,
        device.clone(),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let session = as_session(&body);
    let vault_id = session.vaults[0].vault_id.clone();
    push(
        &app.router,
        &session.session_token,
        &vault_id,
        "envelope-one",
    )
    .await;

    let dir = temp_dir();
    let master_key =
        base64::engine::general_purpose::STANDARD.encode(app.state.config().server_master_key);
    settings::write(
        &dir,
        &FileConfig {
            database_url: Some(base_url()),
            server_master_key: Some(master_key.clone()),
            backup: Some(file_backup(&s3, 2)),
            ..Default::default()
        },
    )
    .unwrap();
    let backups = Backups::with_kdf(dir.clone(), FAST);

    // First scheduled run uploads.
    let first = backups.run(&app.pool, Trigger::Scheduled).await.unwrap();
    let Outcome::Uploaded { key: first_key, .. } = first.clone() else {
        panic!("expected an upload, got {first:?}");
    };
    assert!(
        first_key.starts_with("srv1/ourshell-backup-"),
        "{first_key}"
    );
    let st = backups.state();
    assert_eq!(st.last_result.as_deref(), Some("uploaded"));
    assert_eq!(st.last_key.as_deref(), Some(first_key.as_str()));

    // Nothing changed: skipped, nothing uploaded.
    let again = backups.run(&app.pool, Trigger::Scheduled).await.unwrap();
    assert_eq!(
        again,
        Outcome::Unchanged {
            key: first_key.clone()
        }
    );
    assert_eq!(backups.state().last_result.as_deref(), Some("unchanged"));
    // Signing in again on the same device (new session + audit event) is
    // churn, not a change.
    let (s, _) = login_user(
        &app.router,
        "owner@example.com",
        "owner pw",
        None,
        device.clone(),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(matches!(
        backups.run(&app.pool, Trigger::Scheduled).await.unwrap(),
        Outcome::Unchanged { .. }
    ));
    assert_eq!(backup_keys(&client.list("srv1/").await.unwrap()).len(), 1);

    // Real changes produce new files; retention keeps the newest 2.
    let mut keys = vec![first_key.clone()];
    for i in 0..2 {
        tokio::time::sleep(Duration::from_millis(1100)).await; // distinct file names
        push(
            &app.router,
            &session.session_token,
            &vault_id,
            &format!("envelope-{i}"),
        )
        .await;
        match backups.run(&app.pool, Trigger::Scheduled).await.unwrap() {
            Outcome::Uploaded { key, deleted, .. } => {
                assert_eq!(deleted.is_empty(), i == 0, "{deleted:?}");
                keys.push(key);
            }
            other => panic!("expected an upload, got {other:?}"),
        }
    }
    let remote = backup_keys(&client.list("srv1/").await.unwrap());
    assert_eq!(remote, keys[1..].to_vec());
    // A manual run uploads even when nothing changed.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(matches!(
        backups.run(&app.pool, Trigger::Manual).await.unwrap(),
        Outcome::Uploaded { .. }
    ));
    let latest = backup_keys(&client.list("srv1/").await.unwrap())
        .pop()
        .unwrap();

    // Download and decrypt the latest; a wrong passphrase fails.
    let file = client.get_object(&latest).await.unwrap();
    assert!(file.starts_with(format::MAGIC));
    assert!(format::open(&file, "not the passphrase").is_err());
    let bundle = format::open(&file, PASSPHRASE).unwrap();
    assert!(bundle.config_toml.contains(&master_key));
    let users = bundle
        .manifest
        .tables
        .iter()
        .find(|t| t.name == "users")
        .unwrap();
    assert_eq!(users.rows, 1);
    assert!(!bundle.tables.iter().any(|(t, _)| t == "_sqlx_migrations"));

    // A backup from a newer server is refused.
    let migrator = ourshell_server::migrator().await.unwrap();
    let (fresh, fresh_schema) = empty_schema().await;
    let mut newer = bundle.clone();
    newer.manifest.migrations.push(99_999_999);
    assert!(restore::restore_database(&fresh, &migrator, &newer)
        .await
        .is_err());

    // Restore into an empty schema: same rows, and the user can sign in.
    let loaded = restore::restore_database(&fresh, &migrator, &bundle)
        .await
        .unwrap();
    for (table, rows) in &loaded {
        assert_eq!(
            count(&fresh, table).await,
            count(&app.pool, table).await,
            "{table}"
        );
        assert_eq!(
            Some(*rows),
            bundle
                .manifest
                .tables
                .iter()
                .find(|t| &t.name == table)
                .map(|t| t.rows)
        );
    }
    assert_eq!(count(&fresh, "items").await, 3);
    let restored_cfg: FileConfig = toml::from_str(&bundle.config_toml).unwrap();
    let mut config = Config::from_effective(&settings::Effective {
        values: restored_cfg,
        env_locked: vec![],
    })
    .unwrap()
    .unwrap();
    config.rate_limit_per_min = 1000;
    let state = ourshell_server::state::AppState::new(fresh.clone(), config)
        .await
        .unwrap();
    let restored = ourshell_server::routes::build_router(state);
    let (s, body) = login_user(
        &restored,
        "owner@example.com",
        "owner pw",
        None,
        new_device("d1"),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let token = as_session(&body).session_token;
    let (s, body) = request::<()>(
        &restored,
        "GET",
        &format!("/v1/vaults/{vault_id}/items?since=0"),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let pulled: PullResponse = serde_json::from_value(body).unwrap();
    let mut envelopes: Vec<String> = pulled.items.into_iter().map(|i| i.envelope).collect();
    envelopes.sort();
    assert_eq!(envelopes, vec!["envelope-0", "envelope-1", "envelope-one"]);
    // New rows continue after the restored serial values.
    push(&restored, &token, &vault_id, "after-restore").await;

    fresh.close().await;
    app.pool.close().await;
    drop_schemas(&[fresh_schema, source_schema]).await;
    for k in backup_keys(&client.list("").await.unwrap()) {
        client.delete_object(&k).await.unwrap();
    }
    std::fs::remove_dir_all(dir).unwrap();
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    cookie: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, cookie)
        .header("x-ourshell-admin", "1")
        .body(body.map_or(Body::empty(), |b| Body::from(b.to_string())))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn admin_backups_tab() {
    let hash = password::hash("admin password 1").unwrap();
    let Some(app) = common::setup_with(|c| c.admin_password_hash = Some(hash)).await else {
        return;
    };
    let Some(s3) = minio_bucket().await else {
        return;
    };
    let dir = temp_dir();
    let web = admin::router(admin::AdminCtx {
        state: app.state.clone(),
        data_dir: dir.clone(),
        sessions: Arc::new(admin::Sessions::default()),
        reload: Reload::default(),
        env_locked: Arc::new(vec![]),
        tls_status: Arc::new(Default::default()),
        backups: Backups::with_kdf(dir.clone(), FAST),
    });
    let (s, _) = call(&web, "GET", "/admin/api/backup", "", None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let req = Request::builder()
        .method("POST")
        .uri("/admin/api/login")
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-ourshell-admin", "1")
        .body(Body::from(
            json!({"password": "admin password 1"}).to_string(),
        ))
        .unwrap();
    let res = web.clone().oneshot(req).await.unwrap();
    let cookie = res.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    let (s, body) = call(&web, "GET", "/admin/api/backup", &cookie, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["settings"]["enabled"], false);
    assert_eq!(body["settings"]["interval_hours"], 24);
    assert_eq!(body["settings"]["keep"], 30);

    let mut form = json!({
        "enabled": true,
        "endpoint": s3.endpoint,
        "region": "us-east-1",
        "bucket": s3.bucket,
        "prefix": "admin-test",
        "access_key_id": s3.access_key_id,
        "secret_access_key": s3.secret_access_key,
        "path_style": true,
        "passphrase": "too short",
        "interval_hours": 12,
        "keep": 5,
    });
    let (s, body) = call(
        &web,
        "PUT",
        "/admin/api/backup",
        &cookie,
        Some(form.clone()),
    )
    .await;
    assert_eq!(
        (s, body["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("weak_passphrase"))
    );
    // Testing the unsaved form: a wrong secret is reported, the right one works.
    form["passphrase"] = json!(PASSPHRASE);
    form["secret_access_key"] = json!("wrong-secret");
    let (s, body) = call(
        &web,
        "POST",
        "/admin/api/backup/test",
        &cookie,
        Some(form.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("SignatureDoesNotMatch"),
        "{body}"
    );
    form["secret_access_key"] = json!(s3.secret_access_key);
    let (s, body) = call(
        &web,
        "POST",
        "/admin/api/backup/test",
        &cookie,
        Some(form.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");

    let (s, body) = call(
        &web,
        "PUT",
        "/admin/api/backup",
        &cookie,
        Some(form.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    // Secrets are never returned; saving with them empty keeps them.
    let (_, body) = call(&web, "GET", "/admin/api/backup", &cookie, None).await;
    let text = body.to_string();
    assert!(!text.contains(PASSPHRASE) && !text.contains(&s3.secret_access_key));
    assert_eq!(body["settings"]["passphrase_set"], true);
    assert_eq!(body["settings"]["secret_access_key_set"], true);
    assert_eq!(body["settings"]["prefix"], "admin-test/");
    form["passphrase"] = json!("");
    form["secret_access_key"] = json!(null);
    form["keep"] = json!(4);
    let (s, _) = call(&web, "PUT", "/admin/api/backup", &cookie, Some(form)).await;
    assert_eq!(s, StatusCode::OK);
    let saved = settings::read(&dir).unwrap().backup.unwrap();
    assert_eq!(saved.passphrase.as_deref(), Some(PASSPHRASE));
    assert_eq!(saved.secret_access_key, Some(s3.secret_access_key.clone()));
    assert_eq!(saved.keep, Some(4));

    // Back up now, then it shows in the status and the list.
    let (s, body) = call(&web, "POST", "/admin/api/backup/run", &cookie, None).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["outcome"]["result"], "uploaded");
    let key = body["outcome"]["key"].as_str().unwrap().to_string();
    assert_eq!(body["status"]["last_result"], "uploaded");
    let (s, body) = call(&web, "GET", "/admin/api/backup/list", &cookie, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["backups"][0]["key"], key.as_str());
    assert!(body["backups"][0]["size"].as_u64().unwrap() > 100);
    let (_, body) = call(&web, "GET", "/admin/api/backup", &cookie, None).await;
    assert!(body["status"]["last_success_at"].is_string());
    assert!(!body.to_string().contains(PASSPHRASE));

    let client = S3Client::new(s3).unwrap();
    for k in backup_keys(&client.list("").await.unwrap()) {
        client.delete_object(&k).await.unwrap();
    }
    let schema = current_schema(&app.pool).await;
    app.pool.close().await;
    drop_schemas(&[schema]).await;
    std::fs::remove_dir_all(dir).unwrap();
}
