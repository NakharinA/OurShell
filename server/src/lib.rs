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
pub mod settings;
pub mod sqlx;
pub mod state;
pub mod tls;
pub mod web;

use std::future::IntoFuture;
use std::net::SocketAddr;
use std::path::Path;

use anyhow::{Context, Result};
use axum::serve::ListenerExt;

use crate::config::{Config, DEFAULT_BIND_ADDR};
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

/// Serves until the process is stopped. Starts in setup mode (the `/setup`
/// wizard) when there is no database URL or master key yet, and rebuilds
/// the whole app in place, on the same socket, whenever the setup wizard or
/// the admin page changes the configuration.
pub async fn run() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let data_dir = settings::data_dir();
    let bind_addr = settings::load(&data_dir)?
        .values
        .bind_addr
        .unwrap_or_else(|| DEFAULT_BIND_ADDR.to_string());
    let addr: SocketAddr = bind_addr
        .parse()
        .with_context(|| format!("BIND_ADDR {bind_addr:?} is not a valid socket address"))?;
    let listener = SharedListener(std::sync::Arc::new(
        tokio::net::TcpListener::bind(addr)
            .await
            .with_context(|| format!("failed to listen on {addr}"))?,
    ));
    tracing::info!(%addr, data_dir = %data_dir.display(), "ourshell-server listening");

    // Optional built-in HTTPS (see `tls`): its socket is bound the first
    // time a generation needs it and then shared like `listener`.
    let env = |k: &str| std::env::var(k).ok();
    let mut tls_socket = tls::TlsSocket::new(tls::bind_addr(&env)?);
    let acme_staging = tls::acme_staging(&env);
    let tls_status = std::sync::Arc::new(tls::TlsStatus::default());

    let reload = web::Reload::default();
    let sessions = std::sync::Arc::new(web::admin::Sessions::default());
    loop {
        let effective = settings::load(&data_dir)?;
        let changed = reload.subscribe();
        let config = Config::from_effective(&effective)?;
        let wanted_tls = tls::wanted(config.as_ref());
        let app = match config {
            Some(config) => {
                let state = build_state(config).await?;
                routes::build_router(state.clone()).merge(web::admin::router(
                    web::admin::AdminCtx {
                        state,
                        data_dir: data_dir.clone(),
                        sessions: sessions.clone(),
                        reload: reload.clone(),
                        env_locked: std::sync::Arc::new(effective.env_locked.clone()),
                        tls_status: tls_status.clone(),
                    },
                ))
            }
            None => {
                let code = settings::setup_code(&data_dir)?;
                tracing::warn!(
                    "this server is not set up yet: open http://<this-host>:{}/setup in a browser and enter the setup code {code}",
                    addr.port()
                );
                web::setup::router(web::setup::SetupCtx::new(
                    data_dir.clone(),
                    code,
                    effective,
                    reload.clone(),
                ))
            }
        };
        // Started per generation, so a changed or removed TLS domain takes
        // effect on reload; dropping it stops the ACME renewal task.
        let mut tls_gen = tls::start(
            &mut tls_socket,
            wanted_tls.as_ref(),
            &data_dir,
            acme_staging,
            &tls_status,
        )
        .await;
        let shutdown = |mut changed: tokio::sync::watch::Receiver<u64>| async move {
            let _ = changed.changed().await;
        };
        // `tap_io` (a no-op) because axum provides `SocketAddr` connect
        // info, which the rate limiter needs, for `TapIo` listeners.
        let http = axum::serve(
            listener.clone().tap_io(|_| {}),
            app.clone()
                .into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown(changed.clone()));
        match tls_gen.listener.take() {
            Some(tls_listener) => {
                let https = axum::serve(
                    tls_listener.tap_io(|_| {}),
                    app.layer(axum::Extension(tls::ViaTls))
                        .into_make_service_with_connect_info::<SocketAddr>(),
                )
                .with_graceful_shutdown(shutdown(changed));
                tokio::try_join!(http.into_future(), https.into_future())?;
            }
            None => http.await?,
        }
        drop(tls_gen);
        tracing::info!("configuration changed; reloading");
    }
}

/// The one bound socket, shared by every serve generation across reloads.
/// (Duplicating the socket handle instead doesn't work on Windows: tokio
/// gets no readiness events for the copy.)
#[derive(Clone)]
struct SharedListener(std::sync::Arc<tokio::net::TcpListener>);

impl axum::serve::Listener for SharedListener {
    type Io = tokio::net::TcpStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.0.accept().await {
                Ok(conn) => return conn,
                // Like axum's own listener: e.g. out of file descriptors.
                Err(e) => {
                    tracing::warn!(error = %e, "accept failed");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.0.local_addr()
    }
}

/// `ourshell-server healthcheck`: exit status for Docker's HEALTHCHECK
/// (the runtime image has no curl).
pub async fn healthcheck() -> Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let bind = settings::load(&settings::data_dir())?
        .values
        .bind_addr
        .unwrap_or_else(|| DEFAULT_BIND_ADDR.to_string());
    let port = bind.rsplit_once(':').map_or("8443", |(_, p)| p);
    let mut stream = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")),
    )
    .await
    .context("timed out")??;
    stream
        .write_all(b"GET /v1/health HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .await?;
    let mut response = String::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        stream.read_to_string(&mut response),
    )
    .await
    .context("timed out")??;
    let status = response.lines().next().unwrap_or_default();
    anyhow::ensure!(status.contains(" 200 "), "unhealthy: {status}");
    Ok(())
}

/// `ourshell-server set-admin-password`: sets the `/admin` password from
/// stdin (for env-only deployments, or a forgotten password). Restart the
/// server to apply it.
pub fn set_admin_password() -> Result<()> {
    let password = match std::env::var("OURSHELL_ADMIN_PASSWORD") {
        Ok(p) if !p.is_empty() => p,
        _ => {
            eprint!("New admin password: ");
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            line.trim_end_matches(['\r', '\n']).to_string()
        }
    };
    anyhow::ensure!(
        password.chars().count() >= web::password::MIN_LEN,
        "the password needs at least {} characters",
        web::password::MIN_LEN
    );
    let dir = settings::data_dir();
    let mut file = settings::read(&dir)?;
    file.admin_password_hash = Some(web::password::hash(&password)?);
    settings::write(&dir, &file)?;
    eprintln!("Saved. Restart the server to apply it.");
    Ok(())
}
