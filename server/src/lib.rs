//! OurShell zero-knowledge sync server.
//!
//! The server never holds a key that can decrypt user data (see
//! `docs/threat-model.md`). It stores OPAQUE registration records, public
//! keys, sealed/wrapped key material and item ciphertext, and layers a
//! second, independent envelope encryption over `items.envelope` and
//! `totp.secret` to protect stolen backups (`SERVER_MASTER_KEY`).

pub mod auth;
pub mod backup;
pub mod config;
pub mod crypto;
pub mod db;
pub mod error;
pub mod oidc;
pub mod proxy;
pub mod rate_limit;
pub mod routes;
pub mod settings;
pub mod sqlx;
pub mod startup;
pub mod state;
pub mod tls;
pub mod web;

use std::future::IntoFuture;
use std::net::SocketAddr;
use std::path::Path;

use anyhow::{Context, Result};
use axum::serve::ListenerExt;

use crate::config::DEFAULT_BIND_ADDR;
use crate::sqlx::migrate::Migrator;
use crate::state::AppState;

/// This server's database migrations.
///
/// Runtime migration loading (reads `migrations/*.sql` off disk at startup)
/// rather than the `sqlx::migrate!` compile-time macro -- see `src/sqlx.rs`
/// for why this crate avoids the `sqlx` facade's "macros" feature entirely.
///
/// `MIGRATIONS_DIR` overrides the location (the Docker image sets it to
/// where the Dockerfile copied `server/migrations/`, since the
/// `CARGO_MANIFEST_DIR` default below is a build-time path that only exists
/// on the machine that compiled the binary).
pub async fn migrator() -> Result<Migrator> {
    let migrations_dir = std::env::var("MIGRATIONS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations"));
    Migrator::new(migrations_dir.as_path())
        .await
        .context("failed to load migrations directory")
}

/// Serves until the process is stopped. [`startup::decide`] picks the app
/// or the setup wizard (`/setup`); the whole app is rebuilt in place, on
/// the same socket, whenever the setup wizard or the admin page changes
/// the configuration (or an unreachable database comes back).
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
    let acme_directory = tls::directory_url(tls::acme_staging(&env));
    let tls_status = std::sync::Arc::new(tls::TlsStatus::default());

    let reload = web::Reload::default();
    let sessions = std::sync::Arc::new(web::admin::Sessions::default());
    let backups = backup::Backups::new(data_dir.clone());
    let remembered = startup::Remembered::default();
    loop {
        let changed = reload.subscribe();
        // Deciding can take a while (up to a minute waiting for the
        // database); meanwhile answer health checks, so an orchestrator's
        // liveness probe doesn't restart the server, and show a short page.
        let (decided, decided_rx) = tokio::sync::oneshot::channel::<()>();
        let starting = tokio::spawn(
            axum::serve(
                listener.clone().tap_io(|_| {}),
                web::starting_router().into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                let _ = decided_rx.await;
            })
            .into_future(),
        );
        let decision = startup::decide(
            &data_dir,
            &settings::process_env,
            &remembered,
            startup::CONNECT_WAIT,
        )
        .await;
        let _ = decided.send(());
        let _ = starting.await;
        let decision = decision?;
        // Per-generation tasks, like the TLS ones below; dropping them on
        // reload stops them.
        let mut backup_schedule = None;
        let mut db_retry = None;
        let (app, wanted_tls) = match decision {
            startup::Decision::Ready(ready) => {
                let startup::Ready {
                    config,
                    effective,
                    pool,
                    key_from_env,
                } = *ready;
                if !key_from_env {
                    warn_if_key_not_persistent(&data_dir);
                }
                let wanted_tls = tls::wanted(Some(&config));
                let state = AppState::new(pool, config).await?;
                let ctx = web::admin::AdminCtx {
                    state: state.clone(),
                    data_dir: data_dir.clone(),
                    sessions: sessions.clone(),
                    reload: reload.clone(),
                    effective: std::sync::Arc::new(effective),
                    tls_status: tls_status.clone(),
                    backups: backups.clone(),
                };
                backup_schedule = Some(backups.start(ctx.store()));
                tracing::info!("ready");
                (
                    routes::build_router(state).merge(web::admin::router(ctx)),
                    wanted_tls,
                )
            }
            startup::Decision::Setup(setup_state) => {
                let code = settings::setup_code(&data_dir)?;
                let why = match setup_state.phase {
                    startup::Phase::NoDatabase => "this server is not set up yet".to_string(),
                    startup::Phase::New => "the database has no OurShell data yet".to_string(),
                    startup::Phase::Unreachable => format!(
                        "can't reach the database{} ({}); retrying every {} s",
                        setup_state
                            .database
                            .as_deref()
                            .map(|d| format!(" at {d}"))
                            .unwrap_or_default(),
                        setup_state.error.as_deref().unwrap_or("unknown error"),
                        startup::RETRY_EVERY.as_secs()
                    ),
                    startup::Phase::ExistingData => match setup_state.key_status {
                        startup::KeyStatus::Wrong => "the master key doesn't match the existing data in this database; nothing was changed".to_string(),
                        _ => "the database has existing OurShell data but no master key is configured (set SERVER_MASTER_KEY, or import it in the wizard)".to_string(),
                    },
                };
                tracing::warn!(
                    "{why}: open http://<this-host>:{}/setup in a browser and enter the setup code {code}",
                    addr.port()
                );
                if setup_state.phase == startup::Phase::Unreachable {
                    if let Some(url) = setup_state.database_url.clone() {
                        db_retry = Some(startup::retry_until_reachable(url, reload.clone()));
                    }
                }
                let effective =
                    startup::Bootstrap::load(&data_dir, &settings::process_env, &remembered)?
                        .effective;
                (
                    web::setup::router(
                        web::setup::SetupCtx::new(
                            data_dir.clone(),
                            code,
                            effective,
                            reload.clone(),
                        )
                        .with_startup(*setup_state, remembered.clone()),
                    ),
                    None,
                )
            }
        };
        // Started per generation, so a changed or removed TLS domain takes
        // effect on reload; dropping it stops the ACME renewal task.
        let mut tls_gen = tls::start(
            &mut tls_socket,
            wanted_tls.as_ref(),
            &data_dir,
            acme_directory,
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
        drop(backup_schedule);
        drop(db_retry);
        tracing::info!("configuration changed; reloading");
    }
}

/// Logged at every startup when the master key isn't in the environment
/// and the data dir (where `config.toml` keeps it) won't survive a
/// redeploy.
fn warn_if_key_not_persistent(data_dir: &Path) {
    let p = startup::data_dir_persistence(data_dir);
    if p.persistent {
        return;
    }
    tracing::warn!(
        "SERVER_MASTER_KEY is not set in the environment and {}: the next redeploy loses the \
         master key, and without it the database can't be read. Copy server_master_key from \
         {} into the SERVER_MASTER_KEY environment variable (and DATABASE_URL too, if it isn't \
         set there), or mount a volume at the data dir.",
        p.detail,
        data_dir.join(settings::CONFIG_FILE).display()
    );
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
pub async fn set_admin_password() -> Result<()> {
    let password = match std::env::var("OURSHELL_ADMIN_PASSWORD") {
        Ok(p) if !p.is_empty() => p,
        _ => {
            eprint!("New admin password: ");
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            line.trim_end_matches(['\r', '\n']).to_string()
        }
    };
    set_admin_password_in(&settings::data_dir(), &password, &settings::process_env).await?;
    eprintln!("Saved. Restart the server to apply it.");
    Ok(())
}

/// Stores a new `/admin` password hash in the database's settings (and
/// removes a stale one from `config.toml`, which would override it).
/// Needs the database URL and master key (environment or `config.toml`).
pub async fn set_admin_password_in(
    data_dir: &Path,
    password: &str,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<()> {
    anyhow::ensure!(
        password.chars().count() >= web::password::MIN_LEN,
        "the password needs at least {} characters",
        web::password::MIN_LEN
    );
    let boot = startup::Bootstrap::load(data_dir, env, &startup::Remembered::default())?;
    let (Some(url), Some(key)) = (boot.database_url.clone(), boot.key()?) else {
        anyhow::bail!(
            "set-admin-password needs the database URL and the master key: set DATABASE_URL and \
             SERVER_MASTER_KEY (or finish the setup wizard first)"
        );
    };
    let pool = startup::connect_once(&url)
        .await
        .map_err(|e| anyhow::anyhow!("can't connect to the database: {e}"))?;
    anyhow::ensure!(
        startup::check_key(&pool, &key).await? != startup::KeyStatus::Wrong,
        "SERVER_MASTER_KEY doesn't match this database"
    );
    migrator()
        .await?
        .run(&pool)
        .await
        .context("failed to run database migrations")?;
    let hash = web::password::hash(password)?;
    let mut db = settings::db::read(&pool, &key).await?.unwrap_or_default();
    db.admin_password_hash = Some(hash);
    settings::db::write(&pool, &key, &db).await?;
    pool.close().await;
    let mut file = settings::read(data_dir)?;
    if file.admin_password_hash.take().is_some() {
        settings::write(data_dir, &file)?;
        eprintln!("Removed the old admin password hash from config.toml.");
    }
    Ok(())
}
