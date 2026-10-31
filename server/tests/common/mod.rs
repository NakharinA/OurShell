//! Shared integration test harness.
//!
//! Drives the real axum `Router` in-process with `tower::ServiceExt::oneshot`
//! (no bound socket needed) against an isolated Postgres *schema* per test,
//! so tests can run concurrently against one shared `postgres:17` container.
//!
//! If no Postgres is reachable, [`setup`] prints why and returns `None`;
//! every test using it does the same and passes trivially, so
//! `cargo test --workspace` never hard-fails on a machine without Postgres.
//!
//! Each `tests/*.rs` integration test binary compiles this module fresh via
//! `mod common;` and only a subset of them exercise every helper (e.g. only
//! `auth_flow.rs` logs back in, only the at-rest test queries `pool`
//! directly), so `dead_code` is expected per-binary and silenced here.
#![allow(dead_code)]

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use ourshell_proto::opaque;
use ourshell_proto::sync::{
    AccountKeysUpload, DeviceRegister, LoginFinish, LoginStart, LoginStartResponse, RegisterFinish,
    RegisterStart, RegisterStartResponse, SessionResponse, VaultUpload,
};
use ourshell_server::config::Config;
use ourshell_server::routes::build_router;
use ourshell_server::state::AppState;
use rand::RngCore;
use serde::Serialize;
use serde_json::Value;
use sqlx_core::executor::Executor;
use sqlx_core::sql_str::AssertSqlSafe;
use sqlx_postgres::{PgPool, PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

pub const DEFAULT_TEST_DATABASE_URL: &str = "postgres://postgres:pw@127.0.0.1:55432/postgres";

pub struct TestApp {
    pub router: Router,
    /// Connected with the test's isolated schema already on its search_path,
    /// so raw queries here (e.g. at-rest checks) see the same rows the app does.
    pub pool: PgPool,
    /// The state behind `router`, for building extra routers (e.g. the
    /// admin page) over the same database.
    pub state: AppState,
}

/// Spins up a fresh, isolated Postgres schema, runs migrations into it, and
/// builds the real router over it. `None` if no test database is reachable.
pub async fn setup() -> Option<TestApp> {
    setup_with(|_| {}).await
}

/// Like [`setup`] but lets the caller tweak the config (e.g. `invite_only`)
/// before the `AppState`/router are built.
pub async fn setup_with(configure: impl FnOnce(&mut Config)) -> Option<TestApp> {
    let base_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| DEFAULT_TEST_DATABASE_URL.to_string());

    let probe = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(3))
        .connect(&base_url)
        .await;
    let probe_pool = match probe {
        Ok(p) => p,
        Err(e) => {
            println!(
                "skipping: no test Postgres reachable at {base_url} ({e}); \
                 run `docker run -d --name ourshell-pg-test -e POSTGRES_PASSWORD=pw -p 55432:5432 postgres:17`"
            );
            return None;
        }
    };

    let schema = format!("test_{}", Uuid::new_v4().simple());
    let create_schema_sql = format!("CREATE SCHEMA \"{schema}\"");
    probe_pool
        .execute(AssertSqlSafe(create_schema_sql))
        .await
        .expect("failed to create test schema");
    probe_pool.close().await;

    let schema_for_hook = schema.clone();
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .after_connect(move |conn, _meta| {
            let schema = schema_for_hook.clone();
            Box::pin(async move {
                let set_search_path_sql = format!("SET search_path = \"{schema}\"");
                conn.execute(AssertSqlSafe(set_search_path_sql)).await?;
                Ok(())
            })
        })
        .connect(&base_url)
        .await
        .expect("failed to connect the per-test pool");

    let migrations_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let migrator = sqlx_core::migrate::Migrator::new(migrations_dir.as_path())
        .await
        .expect("failed to load migrations directory");
    migrator.run(&pool).await.expect("failed to run migrations");

    let mut config = test_config(base_url);
    configure(&mut config);

    let state = AppState::new(pool.clone(), config)
        .await
        .expect("failed to build AppState");
    let router = build_router(state.clone());

    Some(TestApp {
        router,
        pool,
        state,
    })
}

fn test_config(database_url: String) -> Config {
    let mut server_master_key = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut server_master_key);
    Config {
        database_url,
        bind_addr: "127.0.0.1:0".to_string(),
        server_master_key,
        signups_enabled: true,
        invite_only: false,
        max_devices_per_user: 10,
        rate_limit_per_min: 1000,
        session_ttl_hours: 24,
        oidc: None,
        public_url: None,
        tls: None,
        proxy: Default::default(),
        admin_password_hash: None,
    }
}

/// Sends one JSON request through the router and returns `(status, body)`.
/// `body` is `Value::Null` for empty/non-JSON responses.
pub async fn request<T: Serialize>(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&T>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(path);
    builder = builder.header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let request_body = match body {
        Some(b) => Body::from(serde_json::to_vec(b).unwrap()),
        None => Body::empty(),
    };
    let req = builder.body(request_body).unwrap();

    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

pub fn new_device(name: &str) -> DeviceRegister {
    DeviceRegister {
        device_id: Uuid::new_v4().to_string(),
        name: name.to_string(),
        public_key: "test-device-public-key".to_string(),
    }
}

/// Drives a full OPAQUE registration over HTTP using the real client
/// helpers from `ourshell_proto::opaque`, exactly as a real client would.
pub async fn register_user(
    app: &Router,
    email: &str,
    password: &str,
    invite_code: Option<&str>,
    device: DeviceRegister,
) -> (StatusCode, Value) {
    let (client_state, request_bytes) = opaque::client_register_start(password.as_bytes()).unwrap();
    let (status, body) = request(
        app,
        "POST",
        "/v1/register/start",
        None,
        Some(&RegisterStart {
            email: email.to_string(),
            registration_request: request_bytes,
            invite_code: invite_code.map(|s| s.to_string()),
        }),
    )
    .await;
    if status != StatusCode::OK {
        return (status, body);
    }
    let start_resp: RegisterStartResponse = serde_json::from_value(body).unwrap();

    let (upload_bytes, _export_key) = opaque::client_register_finish(
        client_state,
        password.as_bytes(),
        &start_resp.registration_response,
    )
    .unwrap();

    let vault_id = Uuid::new_v4().to_string();
    request(
        app,
        "POST",
        "/v1/register/finish",
        None,
        Some(&RegisterFinish {
            registration_id: start_resp.registration_id,
            registration_upload: upload_bytes,
            account: AccountKeysUpload {
                enc_public: "enc-pub".to_string(),
                sign_public: "sign-pub".to_string(),
                kdf: "{}".to_string(),
                wrapped_account_keys: b"wrapped-account-keys".to_vec(),
                recovery_account_wrap: Vec::new(),
            },
            vault: VaultUpload {
                vault_id,
                key_id: "vk1".to_string(),
                sealed_vault_key: b"sealed-personal-vault-key".to_vec(),
                recovery_wrap: b"recovery-wrap".to_vec(),
            },
            device,
        }),
    )
    .await
}

/// Drives a full OPAQUE login over HTTP, optionally supplying a TOTP code.
pub async fn login_user(
    app: &Router,
    email: &str,
    password: &str,
    totp: Option<&str>,
    device: DeviceRegister,
) -> (StatusCode, Value) {
    let (client_state, request_bytes) = opaque::client_login_start(password.as_bytes()).unwrap();
    let (status, body) = request(
        app,
        "POST",
        "/v1/login/start",
        None,
        Some(&LoginStart {
            email: email.to_string(),
            credential_request: request_bytes,
        }),
    )
    .await;
    if status != StatusCode::OK {
        return (status, body);
    }
    let start_resp: LoginStartResponse = serde_json::from_value(body).unwrap();

    let finish_result = opaque::client_login_finish(
        client_state,
        password.as_bytes(),
        &start_resp.credential_response,
    );
    let Ok((finalization_bytes, _session_key, _export_key)) = finish_result else {
        // Wrong password: the client-side OPAQUE finish itself fails before
        // any finalization is even sent. Surface that to the caller the same
        // way a server-side rejection would read.
        return (
            StatusCode::UNAUTHORIZED,
            serde_json::json!({"error": "invalid_login", "message": "client-side OPAQUE finish failed"}),
        );
    };

    request(
        app,
        "POST",
        "/v1/login/finish",
        None,
        Some(&LoginFinish {
            login_id: start_resp.login_id,
            credential_finalization: finalization_bytes,
            totp: totp.map(|s| s.to_string()),
            device,
        }),
    )
    .await
}

pub fn as_session(body: &Value) -> SessionResponse {
    serde_json::from_value(body.clone()).expect("expected a SessionResponse body")
}
