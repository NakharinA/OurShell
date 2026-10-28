//! OurShell zero-knowledge sync server.
//!
//! The server never holds a key that can decrypt user data (see
//! `docs/threat-model.md`). It stores OPAQUE registration records, public
//! keys, sealed/wrapped key material and item ciphertext, and layers a
//! second, independent envelope encryption over `items.envelope` and
//! `totp.secret` to protect stolen backups (`SERVER_MASTER_KEY`).

pub mod auth;
pub mod config;
pub mod crypto;
pub mod db;
pub mod error;
pub mod oidc;
pub mod rate_limit;
pub mod routes;
pub mod sqlx;
pub mod state;

use std::net::SocketAddr;
use std::path::Path;

use anyhow::{Context, Result};

use crate::config::Config;
use crate::sqlx::{migrate::Migrator, PgPoolOptions};
use crate::state::AppState;

/// Connects to Postgres, runs migrations, and builds the ready-to-serve
/// [`AppState`]. Split out from [`run`] so integration tests can build the
/// router without binding a real socket.
pub async fn build_state(config: Config) -> Result<AppState> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await
        .context("failed to connect to DATABASE_URL")?;
    // Runtime migration loading (reads `migrations/*.sql` off disk at
    // startup) rather than the `sqlx::migrate!` compile-time macro -- see
    // `src/sqlx.rs` for why this crate avoids the `sqlx` facade's "macros"
    // feature entirely.
    //
    // `MIGRATIONS_DIR` overrides the location (the Docker image sets it to
    // where the Dockerfile copied `server/migrations/`, since the
    // `CARGO_MANIFEST_DIR` default below is a build-time path that only
    // exists on the machine that compiled the binary).
    let migrations_dir = std::env::var("MIGRATIONS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations"));
    let migrator = Migrator::new(migrations_dir.as_path())
        .await
        .context("failed to load migrations directory")?;
    migrator
        .run(&pool)
        .await
        .context("failed to run database migrations")?;
    AppState::new(pool, config).await
}

pub async fn run() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = Config::from_env()?;
    let bind_addr = config.bind_addr.clone();
    let state = build_state(config).await?;
    let app = routes::build_router(state);

    let addr: SocketAddr = bind_addr
        .parse()
        .with_context(|| format!("BIND_ADDR {bind_addr:?} is not a valid socket address"))?;
    tracing::info!(%addr, "ourshell-server listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
