//! Boots a real `ourshell-server` (axum `Router`, bound to a real TCP
//! socket so `ourshell_core::sync` can talk to it with real HTTP, unlike
//! the server crate's own tests which drive the router in-process with
//! `tower::ServiceExt::oneshot`) against an isolated Postgres *schema*,
//! mirroring `server/tests/common/mod.rs`'s isolation approach so this can
//! run concurrently with the server crate's own tests against the same
//! shared `postgres:17` container.
//!
//! `start()` returns `None` (printing why) if no test Postgres is
//! reachable, so `cargo test --workspace` never hard-fails on a machine
//! without it; the test using this harness does the same.
#![allow(dead_code)]

use std::time::Duration;

use ourshell_server::config::Config;
use ourshell_server::routes::build_router;
use ourshell_server::state::AppState;
use sqlx_core::executor::Executor;
use sqlx_core::sql_str::AssertSqlSafe;
use sqlx_postgres::{PgPool, PgPoolOptions};
use tokio::net::TcpListener;
use uuid::Uuid;

pub const DEFAULT_TEST_DATABASE_URL: &str = "postgres://postgres:pw@127.0.0.1:55432/postgres";

pub struct TestServer {
    pub base_url: String,
    /// Connected with the test's isolated schema already on its
    /// search_path, so raw queries here (e.g. at-rest checks) see the same
    /// rows the server does.
    pub pool: PgPool,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

/// Spins up a fresh, isolated Postgres schema, runs the server's
/// migrations into it, and serves the real router on a real (OS-assigned)
/// TCP port. `None` if no test database is reachable.
pub async fn start() -> Option<TestServer> {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| DEFAULT_TEST_DATABASE_URL.to_string());

    let probe = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(3))
        .connect(&database_url)
        .await;
    let probe_pool = match probe {
        Ok(p) => p,
        Err(e) => {
            println!(
                "skipping: no test Postgres reachable at {database_url} ({e}); \
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
        .connect(&database_url)
        .await
        .expect("failed to connect the per-test pool");

    let migrations_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/core is two levels under the workspace root")
        .join("server")
        .join("migrations");
    let migrator = sqlx_core::migrate::Migrator::new(migrations_dir.as_path())
        .await
        .expect("failed to load migrations directory");
    migrator.run(&pool).await.expect("failed to run migrations");

    let mut server_master_key = [0u8; 32];
    ourshell_core::crypto::random_bytes(&mut server_master_key)
        .expect("failed to generate a test server master key");
    let config = Config {
        database_url: database_url.clone(),
        bind_addr: "127.0.0.1:0".to_string(),
        server_master_key,
        signups_enabled: true,
        invite_only: false,
        max_devices_per_user: 10,
        rate_limit_per_min: 0, // unlimited: these tests push/pull many requests
        session_ttl_hours: 24,
        oidc: None,
        public_url: None,
        tls: None,
        proxy: Default::default(),
        admin_password_hash: None,
    };

    let state = AppState::new(pool.clone(), config)
        .await
        .expect("failed to build AppState");
    let app = build_router(state);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind a test TCP listener");
    let addr = listener.local_addr().expect("local_addr");
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app.into_make_service())
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await;
    });

    Some(TestServer {
        base_url: format!("http://{addr}"),
        pool,
        shutdown: Some(shutdown_tx),
    })
}
