//! What to serve: the app or the setup wizard. Decided at startup and on
//! every reload (see [`decide`]):
//!
//! | database URL | reachable (~60 s) | database holds          | master key | serves |
//! |--------------|-------------------|-------------------------|------------|--------|
//! | none         | -                 | -                       | -          | wizard |
//! | yes          | no                | -                       | -          | wizard, showing the error; retried every 10 s |
//! | yes          | yes               | nothing of ours (fresh) | any        | wizard (database step done) |
//! | yes          | yes               | OurShell data           | missing    | wizard, "existing data" (import / clear) |
//! | yes          | yes               | OurShell data           | wrong      | wizard, "existing data" + "key doesn't match" |
//! | yes          | yes               | OurShell data           | right      | the app, settings from the database |
//!
//! Nothing here ever changes or clears data on its own: clearing is an
//! explicit, confirmed action in the wizard ([`clear`]).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::config::{self, Config};
use crate::settings::{self, Effective, FileConfig};
use crate::sqlx::{self, AssertSqlSafe, PgPool, PgPoolOptions};

/// How long startup waits for the database (Postgres often starts after
/// the server).
pub const CONNECT_WAIT: Duration = Duration::from_secs(60);
/// While the database is unreachable, setup mode re-checks this often.
pub const RETRY_EVERY: Duration = Duration::from_secs(10);

/// Every table OurShell's migrations create, plus sqlx's bookkeeping
/// table. [`clear`] drops exactly these. (A unit test checks the list
/// against `migrations/*.sql`.)
pub const OURSHELL_TABLES: &[&str] = &[
    "_sqlx_migrations",
    "account_keys",
    "devices",
    "events",
    "invites",
    "items",
    "oidc_requests",
    "opaque_records",
    "orgs",
    "pending_logins",
    "pending_password_changes",
    "pending_registrations",
    "server_settings",
    "server_setup",
    "sessions",
    "totp",
    "users",
    "vault_members",
    "vaults",
];

/// Bootstrap keys set by the wizard this process couldn't (or needn't)
/// write to `config.toml`, so a reload still finds them. Lowest priority,
/// and gone after a restart.
#[derive(Clone, Default)]
pub struct Remembered(Arc<Mutex<FileConfig>>);

impl Remembered {
    pub fn get(&self) -> FileConfig {
        self.0.lock().unwrap().clone()
    }

    pub fn set(&self, boot: &FileConfig) {
        let mut cur = self.0.lock().unwrap();
        *cur = settings::merge(&cur, &boot.bootstrap());
    }
}

/// The bootstrap keys and where they came from.
#[derive(Clone, Debug, Default)]
pub struct Bootstrap {
    pub database_url: Option<String>,
    pub url_from_env: bool,
    /// Base64, as configured (validated by [`Bootstrap::key`]).
    pub master_key: Option<String>,
    pub key_from_env: bool,
    /// Settings from `config.toml` + environment (no database).
    pub effective: Effective,
}

impl Bootstrap {
    /// env > `config.toml` > remembered.
    pub fn load(
        data_dir: &Path,
        env: &dyn Fn(&str) -> Option<String>,
        remembered: &Remembered,
    ) -> Result<Self> {
        let file = settings::read(data_dir)?;
        Self::from_file(&file, env, remembered)
    }

    pub fn from_file(
        file: &FileConfig,
        env: &dyn Fn(&str) -> Option<String>,
        remembered: &Remembered,
    ) -> Result<Self> {
        let mut eff = settings::resolve(file, None, env)?;
        eff.values = settings::merge(&remembered.get(), &eff.values);
        let nonempty = |s: &Option<String>| s.clone().filter(|s| !s.trim().is_empty());
        Ok(Self {
            database_url: nonempty(&eff.values.database_url),
            url_from_env: eff.env_locked.contains(&"database_url"),
            master_key: nonempty(&eff.values.server_master_key),
            key_from_env: eff.env_locked.contains(&"server_master_key"),
            effective: eff,
        })
    }

    pub fn key(&self) -> Result<Option<[u8; 32]>> {
        self.master_key
            .as_deref()
            .map(config::decode_master_key)
            .transpose()
    }
}

/// What a database holds, for the decision and the wizard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DbKind {
    /// No tables at all.
    Empty,
    /// Only other applications' tables (OurShell can live next to them).
    Foreign,
    /// OurShell's tables but nothing set up in them yet.
    Fresh,
    /// An OurShell server's data.
    Existing,
}

impl DbKind {
    /// Setting up a new server here needs no import or clear.
    pub fn is_new(self) -> bool {
        self != DbKind::Existing
    }
}

/// See [`DbKind`]; the rule from the startup table.
pub fn classify(
    ourshell_tables: usize,
    other_tables: usize,
    applied_migrations: usize,
    has_server_setup: bool,
    has_settings: bool,
    users: i64,
) -> DbKind {
    if ourshell_tables == 0 {
        return if other_tables == 0 {
            DbKind::Empty
        } else {
            DbKind::Foreign
        };
    }
    if applied_migrations == 0 || (!has_server_setup && !has_settings && users == 0) {
        return DbKind::Fresh;
    }
    DbKind::Existing
}

#[derive(Clone, Debug, Serialize)]
pub struct Inspection {
    pub kind: DbKind,
    /// PostgreSQL's version string.
    pub server_version: String,
    pub ourshell_tables: Vec<String>,
    /// Tables in the same schema that aren't OurShell's.
    pub other_tables: Vec<String>,
    /// Applied migration versions; the last one is the schema version.
    pub migrations: Vec<i64>,
    pub schema_version: Option<i64>,
    pub accounts: i64,
    pub devices: i64,
    pub items: i64,
    /// When the first org (= the server's first account) was created.
    pub first_org_created: Option<DateTime<Utc>>,
    pub has_server_setup: bool,
    pub has_settings: bool,
}

async fn count_if(pool: &PgPool, tables: &[String], table: &str, sql: &str) -> Result<i64> {
    if !tables.iter().any(|t| t == table) {
        return Ok(0);
    }
    let (n,): (i64,) = sqlx::query_as(AssertSqlSafe(sql.to_string()))
        .fetch_one(pool)
        .await
        .with_context(|| format!("can't read {table}"))?;
    Ok(n)
}

/// Tables in the connection's current schema.
pub async fn list_tables(pool: &PgPool) -> Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT c.relname::text FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = current_schema() AND c.relkind IN ('r', 'p') ORDER BY 1",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(t,)| t).collect())
}

/// Looks at what the database behind `pool` holds. Read-only.
pub async fn inspect(pool: &PgPool) -> Result<Inspection> {
    let (server_version,): (String,) = sqlx::query_as("SELECT version()").fetch_one(pool).await?;
    let tables = list_tables(pool).await?;
    let (ours, other): (Vec<String>, Vec<String>) = tables
        .iter()
        .cloned()
        .partition(|t| OURSHELL_TABLES.contains(&t.as_str()));
    let has = |t: &str| ours.iter().any(|x| x == t);
    let migrations: Vec<i64> = if has("_sqlx_migrations") {
        sqlx::query_as::<_, (i64,)>(
            "SELECT version FROM _sqlx_migrations WHERE success ORDER BY version",
        )
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(v,)| v)
        .collect()
    } else {
        Vec::new()
    };
    let has_server_setup = count_if(
        pool,
        &ours,
        "server_setup",
        "SELECT COUNT(*) FROM server_setup",
    )
    .await?
        > 0;
    let has_settings = count_if(
        pool,
        &ours,
        "server_settings",
        "SELECT COUNT(*) FROM server_settings",
    )
    .await?
        > 0;
    let accounts = count_if(pool, &ours, "users", "SELECT COUNT(*) FROM users").await?;
    let devices = count_if(pool, &ours, "devices", "SELECT COUNT(*) FROM devices").await?;
    let items = count_if(
        pool,
        &ours,
        "items",
        "SELECT COUNT(*) FROM items WHERE NOT deleted",
    )
    .await?;
    let first_org_created = if has("orgs") {
        let (t,): (Option<DateTime<Utc>>,) = sqlx::query_as("SELECT MIN(created_at) FROM orgs")
            .fetch_one(pool)
            .await?;
        t
    } else {
        None
    };
    Ok(Inspection {
        kind: classify(
            ours.len(),
            other.len(),
            migrations.len(),
            has_server_setup,
            has_settings,
            accounts,
        ),
        server_version,
        schema_version: migrations.last().copied(),
        ourshell_tables: ours,
        other_tables: other,
        migrations,
        accounts,
        devices,
        items,
        first_org_created,
        has_server_setup,
        has_settings,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyStatus {
    /// Decrypts the database's `server_setup` (or settings).
    Ok,
    /// Doesn't decrypt it.
    Wrong,
    /// No key configured.
    Missing,
    /// Nothing encrypted to check against (accepted).
    Unverifiable,
}

/// Checks `key` against the database: `server_setup` if there is one, else
/// the settings row.
pub async fn check_key(pool: &PgPool, key: &[u8; 32]) -> Result<KeyStatus> {
    let has_setup: (bool,) = sqlx::query_as("SELECT to_regclass('server_setup') IS NOT NULL")
        .fetch_one(pool)
        .await?;
    if has_setup.0 {
        let row: Option<(Vec<u8>,)> =
            sqlx::query_as("SELECT encrypted_setup FROM server_setup WHERE id = 1")
                .fetch_optional(pool)
                .await?;
        if let Some((blob,)) = row {
            return Ok(
                match crate::crypto::decrypt(key, &blob, crate::state::AAD_SERVER_SETUP) {
                    Ok(_) => KeyStatus::Ok,
                    Err(_) => KeyStatus::Wrong,
                },
            );
        }
    }
    Ok(match settings::db::read_raw(pool).await? {
        Some(blob) => match settings::db::open(key, &blob) {
            Ok(_) => KeyStatus::Ok,
            Err(settings::db::OpenError::WrongKey) => KeyStatus::Wrong,
            Err(e) => return Err(e.into()),
        },
        None => KeyStatus::Unverifiable,
    })
}

/// Drops every OurShell table (and sqlx's migration table) in the
/// connection's schema, in one transaction. Other tables are left alone.
/// Returns the dropped tables.
pub async fn clear(pool: &PgPool) -> Result<Vec<String>> {
    let ours: Vec<String> = list_tables(pool)
        .await?
        .into_iter()
        .filter(|t| OURSHELL_TABLES.contains(&t.as_str()))
        .collect();
    if ours.is_empty() {
        return Ok(ours);
    }
    let list = ours
        .iter()
        .map(|t| crate::backup::dump::quote_ident(t))
        .collect::<Vec<_>>()
        .join(", ");
    let mut tx = pool.begin().await?;
    sqlx::query(AssertSqlSafe(format!(
        "DROP TABLE IF EXISTS {list} CASCADE"
    )))
    .execute(&mut *tx)
    .await
    .context("failed to drop the tables")?;
    tx.commit().await?;
    Ok(ours)
}

/// `secret` hidden in `msg`: the URL's password and the URL itself.
pub fn redact_error(msg: &str, url: &str) -> String {
    let mut out = msg.replace(url, &crate::web::redact_database_url(url));
    if let Some(pw) = url_password(url) {
        out = out.replace(&pw, "***");
    }
    out
}

fn url_password(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let creds = rest.rsplit_once('@')?.0;
    let pw = creds.split_once(':')?.1;
    (!pw.is_empty()).then(|| pw.to_string())
}

/// `postgres://user:<password>@host:5432/db`: what to put in
/// `DATABASE_URL`, without the password.
pub fn database_url_hint(url: &str) -> String {
    let scheme = url.split_once("://").map_or("postgres", |(s, _)| s);
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let user = rest
        .rsplit_once('@')
        .map(|(c, _)| c.split(':').next().unwrap_or_default().to_string())
        .filter(|u| !u.is_empty());
    let host = crate::web::redact_database_url(url);
    match user {
        Some(u) => format!("{scheme}://{u}:<password>@{host}"),
        None => format!("{scheme}://{host}"),
    }
}

fn permanent(e: &sqlx::Error) -> bool {
    match e {
        sqlx::Error::Configuration(_) => true,
        // invalid_authorization_specification / invalid_password
        sqlx::Error::Database(d) => d.code().is_some_and(|c| c.starts_with("28")),
        _ => false,
    }
}

/// One connection attempt (5 s), for the wizard.
pub async fn connect_once(url: &str) -> Result<PgPool, String> {
    let probe = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(5))
        .connect(url)
        .await;
    probe.map_err(|e| redact_error(&e.to_string(), url))
}

/// Connects, retrying for up to `wait` (unless the error can't go away by
/// itself, like a wrong password). Errors are redacted.
pub async fn connect_with_retry(url: &str, wait: Duration) -> Result<PgPool, String> {
    let deadline = Instant::now() + wait;
    let mut logged = false;
    loop {
        let attempt = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(url)
            .await;
        match attempt {
            Ok(probe) => {
                probe.close().await;
                return PgPoolOptions::new()
                    .max_connections(10)
                    .connect(url)
                    .await
                    .map_err(|e| redact_error(&e.to_string(), url));
            }
            Err(e) => {
                let msg = redact_error(&e.to_string(), url);
                if permanent(&e) || Instant::now() >= deadline {
                    return Err(msg);
                }
                if !logged {
                    tracing::warn!(
                        error = %msg,
                        "waiting for the database at {} (up to {} s)",
                        crate::web::redact_database_url(url),
                        wait.as_secs()
                    );
                    logged = true;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

/// Why setup mode is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// No database URL yet: the normal first run.
    NoDatabase,
    /// The database URL is known but the database can't be reached.
    Unreachable,
    /// Connected to a database without OurShell data.
    New,
    /// Connected to an OurShell database, but the master key is missing
    /// or doesn't match it.
    ExistingData,
}

/// What setup mode tells the wizard about the startup decision.
#[derive(Clone, Debug, Serialize)]
pub struct SetupState {
    pub phase: Phase,
    /// `host:port/db`, without credentials.
    pub database: Option<String>,
    pub database_from_env: bool,
    /// Redacted connection error ([`Phase::Unreachable`]).
    pub error: Option<String>,
    pub key_status: KeyStatus,
    pub key_from_env: bool,
    pub inspection: Option<Inspection>,
    /// The full URL, for the background retry. Never sent to the browser.
    #[serde(skip)]
    pub database_url: Option<String>,
}

impl SetupState {
    pub fn no_database(boot: &Bootstrap) -> Self {
        Self {
            phase: Phase::NoDatabase,
            database: None,
            database_from_env: false,
            error: None,
            key_status: if boot.master_key.is_some() {
                KeyStatus::Unverifiable
            } else {
                KeyStatus::Missing
            },
            key_from_env: boot.key_from_env,
            inspection: None,
            database_url: None,
        }
    }

    fn with_db(boot: &Bootstrap, url: &str, phase: Phase) -> Self {
        Self {
            phase,
            database: Some(crate::web::redact_database_url(url)),
            database_from_env: boot.url_from_env,
            database_url: Some(url.to_string()),
            ..Self::no_database(boot)
        }
    }
}

pub struct Ready {
    pub config: Config,
    pub effective: Effective,
    pub pool: PgPool,
    pub key_from_env: bool,
}

pub enum Decision {
    Ready(Box<Ready>),
    Setup(Box<SetupState>),
}

/// The startup decision (see the module docs). Runs migrations only when
/// going ready (existing data + the right key), and moves a 0.8.x
/// `config.toml`'s settings into the database once.
pub async fn decide(
    data_dir: &Path,
    env: &dyn Fn(&str) -> Option<String>,
    remembered: &Remembered,
    connect_wait: Duration,
) -> Result<Decision> {
    let boot = Bootstrap::load(data_dir, env, remembered)?;
    let Some(url) = boot.database_url.clone() else {
        return Ok(Decision::Setup(Box::new(SetupState::no_database(&boot))));
    };
    let key = boot.key()?;
    let pool = match connect_with_retry(&url, connect_wait).await {
        Ok(p) => p,
        Err(e) => {
            let mut st = SetupState::with_db(&boot, &url, Phase::Unreachable);
            st.error = Some(e);
            return Ok(Decision::Setup(Box::new(st)));
        }
    };
    let insp = inspect(&pool).await?;
    if insp.kind.is_new() {
        pool.close().await;
        let mut st = SetupState::with_db(&boot, &url, Phase::New);
        st.inspection = Some(insp);
        return Ok(Decision::Setup(Box::new(st)));
    }
    let status = match &key {
        None => KeyStatus::Missing,
        Some(k) => check_key(&pool, k).await?,
    };
    let (Some(key), KeyStatus::Ok | KeyStatus::Unverifiable) = (key, status) else {
        pool.close().await;
        let mut st = SetupState::with_db(&boot, &url, Phase::ExistingData);
        st.key_status = status;
        st.inspection = Some(insp);
        return Ok(Decision::Setup(Box::new(st)));
    };

    crate::migrator()
        .await?
        .run(&pool)
        .await
        .context("failed to run database migrations")?;
    let file = settings::read(data_dir)?;
    let db = migrate_legacy_settings(data_dir, &file, &pool, &key).await?;
    let file = settings::read(data_dir)?;
    let mut effective = settings::resolve(&file, db.as_ref(), env)?;
    effective.values = settings::merge(&remembered.get(), &effective.values);
    effective.values.database_url = Some(url);
    effective.values.server_master_key = Some(STANDARD.encode(key));
    let config =
        Config::from_effective(&effective)?.context("database URL and master key are set")?;
    Ok(Decision::Ready(Box::new(Ready {
        config,
        effective,
        pool,
        key_from_env: boot.key_from_env,
    })))
}

/// A 0.8.x install keeps every setting in `config.toml`. The first time a
/// server with a settings-free database starts, those settings move into
/// the database, and `config.toml` is rewritten with only the bootstrap
/// keys (the original is kept as `config.toml.before-db-settings`). If the
/// database already has settings, the file's keys keep overriding them
/// (and show read-only on `/admin`). Returns the database's settings.
pub async fn migrate_legacy_settings(
    data_dir: &Path,
    file: &FileConfig,
    pool: &PgPool,
    key: &[u8; 32],
) -> Result<Option<FileConfig>> {
    let db = settings::db::read(pool, key).await?;
    if !file.has_settings() {
        return Ok(db);
    }
    let keys = settings::keys_set(&file.without_bootstrap()).join(", ");
    if db.is_some() {
        tracing::warn!(
            "{} sets {keys}, which override the settings in the database; remove them from the \
             file to manage them on /admin",
            data_dir.join(settings::CONFIG_FILE).display()
        );
        return Ok(db);
    }
    let moved = file.without_bootstrap();
    settings::db::write(pool, key, &moved).await?;
    tracing::info!("moved settings from config.toml into the database: {keys}");
    match settings::update_bootstrap(data_dir, &FileConfig::default(), true) {
        Ok(_) => tracing::info!(
            "config.toml now holds only the database URL / master key / bind address; the \
             original is kept as {}.{}",
            settings::CONFIG_FILE,
            settings::LEGACY_COPY_SUFFIX
        ),
        Err(e) => tracing::warn!(
            error = %format!("{e:#}"),
            "couldn't rewrite config.toml; its settings keep overriding the database until \
             they're removed from it"
        ),
    }
    Ok(Some(moved))
}

/// Re-checks an unreachable database every [`RETRY_EVERY`] and reloads the
/// server once it answers. Dropping the handle stops it.
pub fn retry_until_reachable(url: String, reload: crate::web::Reload) -> RetryTask {
    RetryTask(tokio::spawn(async move {
        loop {
            tokio::time::sleep(RETRY_EVERY).await;
            if let Ok(pool) = connect_once(&url).await {
                pool.close().await;
                tracing::info!("the database is reachable now; reloading");
                reload.trigger_soon();
                return;
            }
        }
    }))
}

pub struct RetryTask(tokio::task::JoinHandle<()>);

impl Drop for RetryTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Whether files in the data dir survive a redeploy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Persistence {
    pub writable: bool,
    /// `false` = the data dir is on the container's own (overlay / tmpfs)
    /// file system, lost when the container is replaced.
    pub persistent: bool,
    pub detail: String,
}

/// Checks `dir`: writable, and (on Linux) mounted from a volume rather
/// than the container's root overlay or a tmpfs. Elsewhere it is assumed
/// persistent.
pub fn data_dir_persistence(dir: &Path) -> Persistence {
    let probe = dir.join(".ourshell-write-test");
    let writable = std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&probe, b"x"))
        .and_then(|()| std::fs::remove_file(&probe))
        .is_ok();
    if !writable {
        return Persistence {
            writable,
            persistent: false,
            detail: format!("{} isn't writable", dir.display()),
        };
    }
    #[cfg(target_os = "linux")]
    {
        let abs = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        if let Ok(info) = std::fs::read_to_string("/proc/self/mountinfo") {
            if let Some(m) = mount_for(&info, &abs) {
                let (persistent, detail) = if m.is_ephemeral_fs() {
                    (
                        false,
                        format!(
                            "{} is on the container's own {} file system, not a volume",
                            abs.display(),
                            m.fstype
                        ),
                    )
                } else if m.is_anonymous_volume() {
                    (
                        false,
                        format!(
                            "{} is an anonymous volume, which a new container doesn't get back; \
                             mount a named volume there",
                            abs.display()
                        ),
                    )
                } else {
                    (
                        true,
                        format!(
                            "{} is on a {} mount at {}",
                            abs.display(),
                            m.fstype,
                            m.point
                        ),
                    )
                };
                return Persistence {
                    writable,
                    persistent,
                    detail,
                };
            }
        }
    }
    Persistence {
        writable,
        persistent: true,
        detail: format!("{} is writable", dir.display()),
    }
}

pub fn is_ephemeral_fs(fstype: &str) -> bool {
    matches!(fstype, "overlay" | "overlayfs" | "aufs" | "tmpfs" | "ramfs")
}

/// One line of `/proc/self/mountinfo`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    /// Where it's mounted in this container.
    pub point: String,
    pub fstype: String,
    /// What's mounted: the path inside the source file system, e.g.
    /// `/var/lib/docker/volumes/<name>/_data` for a Docker volume.
    pub root: String,
}

impl Mount {
    /// The container's own file system (overlay, tmpfs, ...): gone when the
    /// container is replaced.
    pub fn is_ephemeral_fs(&self) -> bool {
        is_ephemeral_fs(&self.fstype)
    }

    /// An *anonymous* Docker volume (`volumes/<64 hex>/_data`), e.g. from the
    /// image's `VOLUME /data` when nothing is mounted there: a redeploy
    /// (a new container) gets a new, empty one.
    pub fn is_anonymous_volume(&self) -> bool {
        let mut parts = self.root.rsplit('/');
        let (Some("_data"), Some(id), Some("volumes")) = (parts.next(), parts.next(), parts.next())
        else {
            return false;
        };
        id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())
    }
}

/// The mount `path` lives on, from `/proc/self/mountinfo` text: the
/// longest mount point that is `path` or one of its ancestors (the last one
/// listed wins for stacked mounts).
pub fn mount_for(mountinfo: &str, path: &Path) -> Option<Mount> {
    let mut best: Option<(usize, Mount)> = None;
    for line in mountinfo.lines() {
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        let fields: Vec<&str> = left.split(' ').collect();
        let (Some(root), Some(point)) = (fields.get(3), fields.get(4)) else {
            continue;
        };
        let Some(fstype) = right.split(' ').next() else {
            continue;
        };
        let point = unescape_mount(point);
        if !path.starts_with(&point) {
            continue;
        }
        let depth = Path::new(&point).components().count();
        if best.as_ref().is_none_or(|(d, _)| depth >= *d) {
            let mount = Mount {
                point,
                fstype: fstype.to_string(),
                root: unescape_mount(root),
            };
            best = Some((depth, mount));
        }
    }
    best.map(|(_, m)| m)
}

/// mountinfo escapes space, tab, newline and backslash as `\ooo`.
fn unescape_mount(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' {
            if let Some(oct) = b.get(i + 1..i + 4) {
                if oct.iter().all(|c| (b'0'..=b'7').contains(c)) {
                    let v = oct
                        .iter()
                        .fold(0u32, |acc, c| acc * 8 + u32::from(c - b'0'));
                    if let Ok(v) = u8::try_from(v) {
                        out.push(v);
                        i += 4;
                        continue;
                    }
                }
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What to add to the environment so a redeploy keeps working, when the
/// data dir won't survive one: `SERVER_MASTER_KEY=...` unless the
/// environment already has it, and a `DATABASE_URL=` hint likewise.
pub fn env_lines(
    boot_url: Option<&str>,
    url_from_env: bool,
    key_b64: Option<&str>,
    key_from_env: bool,
) -> Vec<String> {
    let mut out = Vec::new();
    if let (false, Some(k)) = (key_from_env, key_b64) {
        out.push(format!("SERVER_MASTER_KEY={k}"));
    }
    if let (false, Some(u)) = (url_from_env, boot_url) {
        out.push(format!("DATABASE_URL={}", database_url_hint(u)));
    }
    out
}

/// The CLI commands' view of the settings: `config.toml` + environment,
/// plus the database's settings when it can be reached with the
/// configured URL and master key. Also returns the store for writing.
pub async fn cli_settings(data_dir: &Path) -> Result<(Effective, Option<settings::Store>)> {
    let boot = Bootstrap::load(data_dir, &settings::process_env, &Remembered::default())?;
    let (Some(url), Some(key)) = (boot.database_url.clone(), boot.key()?) else {
        return Ok((boot.effective, None));
    };
    let Ok(pool) = connect_once(&url).await else {
        return Ok((boot.effective, None));
    };
    let db = settings::db::read(&pool, &key).await.ok().flatten();
    let eff = settings::resolve(
        &settings::read(data_dir)?,
        db.as_ref(),
        &settings::process_env,
    )?;
    Ok((
        eff,
        Some(settings::Store {
            data_dir: PathBuf::from(data_dir),
            pool,
            master_key: key,
            database_url: url,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification() {
        assert_eq!(classify(0, 0, 0, false, false, 0), DbKind::Empty);
        assert_eq!(classify(0, 3, 0, false, false, 0), DbKind::Foreign);
        // Tables without a migration record, or nothing set up yet: fresh.
        assert_eq!(classify(5, 0, 0, true, false, 2), DbKind::Fresh);
        assert_eq!(classify(18, 0, 3, false, false, 0), DbKind::Fresh);
        assert_eq!(classify(18, 2, 3, false, false, 0), DbKind::Fresh);
        // Any of server_setup / settings / users: existing data.
        assert_eq!(classify(18, 0, 3, true, false, 0), DbKind::Existing);
        assert_eq!(classify(18, 0, 3, false, true, 0), DbKind::Existing);
        assert_eq!(classify(18, 0, 3, false, false, 1), DbKind::Existing);
        assert!(DbKind::Foreign.is_new() && !DbKind::Existing.is_new());
    }

    #[test]
    fn table_list_covers_the_migrations() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
        let mut found = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let sql = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            for line in sql.lines() {
                if let Some(rest) = line.trim().strip_prefix("CREATE TABLE ") {
                    let name = rest
                        .trim_start_matches("IF NOT EXISTS ")
                        .split([' ', '('])
                        .next()
                        .unwrap()
                        .to_string();
                    found.push(name);
                }
            }
        }
        assert!(found.len() >= 17, "{found:?}");
        for t in &found {
            assert!(OURSHELL_TABLES.contains(&t.as_str()), "{t} missing");
        }
        assert_eq!(found.len() + 1, OURSHELL_TABLES.len(), "{found:?}");
    }

    #[test]
    fn errors_and_hints_hide_the_password() {
        let url = "postgres://ourshell:s3cr%40t@db.internal:5432/ourshell?sslmode=require";
        let msg = format!("error with {url}: password authentication failed (s3cr%40t)");
        let r = redact_error(&msg, url);
        assert!(!r.contains("s3cr%40t"), "{r}");
        assert!(r.contains("db.internal:5432/ourshell"), "{r}");
        assert_eq!(
            database_url_hint(url),
            "postgres://ourshell:<password>@db.internal:5432/ourshell"
        );
        assert_eq!(database_url_hint("postgres://db/x"), "postgres://db/x");
        assert_eq!(
            env_lines(Some(url), false, Some("KEY="), false),
            vec![
                "SERVER_MASTER_KEY=KEY=".to_string(),
                "DATABASE_URL=postgres://ourshell:<password>@db.internal:5432/ourshell".into()
            ]
        );
        assert!(env_lines(Some(url), true, Some("k"), true).is_empty());
    }

    const MOUNTINFO: &str = "\
1078 945 0:243 / / rw,relatime master:500 - overlay overlay rw,lowerdir=/var/lib/docker/...
1079 1078 0:246 / /proc rw,nosuid,nodev,noexec,relatime - proc proc rw
1083 1078 0:248 / /dev rw,nosuid - tmpfs tmpfs rw,size=65536k,mode=755
1090 1078 8:1 /var/lib/docker/volumes/ourshell-data/_data /data rw,relatime - ext4 /dev/sda1 rw
1091 1078 8:1 /var/lib/docker/volumes/x/_data /my\\040data rw,relatime - ext4 /dev/sda1 rw
1092 1078 0:300 / /scratch rw - tmpfs tmpfs rw
1093 1078 8:32 /data/docker/volumes/76252f91b45e1eecf214aee6a4ae632e512af8a6f1bb7b34cb74eed07b43efc0/_data /anon rw,relatime master:61 - ext4 /dev/sdc rw
";

    #[test]
    fn finds_the_mount_of_a_path() {
        let m = |p: &str| {
            let m = mount_for(MOUNTINFO, Path::new(p)).unwrap();
            (m.point, m.fstype)
        };
        assert_eq!(m("/data"), ("/data".into(), "ext4".into()));
        assert_eq!(m("/data/acme"), ("/data".into(), "ext4".into()));
        // `/database` is not under `/data`.
        assert_eq!(m("/database"), ("/".into(), "overlay".into()));
        assert_eq!(m("/app/data"), ("/".into(), "overlay".into()));
        assert_eq!(m("/my data/x"), ("/my data".into(), "ext4".into()));
        assert_eq!(m("/scratch/d"), ("/scratch".into(), "tmpfs".into()));
        assert!(is_ephemeral_fs("overlay") && is_ephemeral_fs("tmpfs"));
        assert!(!is_ephemeral_fs("ext4") && !is_ephemeral_fs("nfs4"));
        // A named volume is kept; an anonymous one (the image's `VOLUME`)
        // isn't, when the container is recreated.
        let named = mount_for(MOUNTINFO, Path::new("/data")).unwrap();
        assert!(!named.is_anonymous_volume() && !named.is_ephemeral_fs());
        let anon = mount_for(MOUNTINFO, Path::new("/anon")).unwrap();
        assert!(anon.is_anonymous_volume(), "{anon:?}");
        // A bare host: `/` on a real disk is persistent.
        let host = "1 0 8:1 / / rw - ext4 /dev/sda1 rw\n";
        let root = mount_for(host, Path::new("/srv/ourshell/data")).unwrap();
        assert_eq!((root.point.as_str(), root.fstype.as_str()), ("/", "ext4"));
        assert!(!root.is_anonymous_volume() && !root.is_ephemeral_fs());
    }

    #[test]
    fn writable_temp_dir_is_persistent_off_linux_containers() {
        let dir = std::env::temp_dir().join(format!("ourshell-persist-{}", uuid::Uuid::new_v4()));
        let p = data_dir_persistence(&dir);
        assert!(p.writable, "{p:?}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn remembered_keeps_only_bootstrap_keys() {
        let r = Remembered::default();
        r.set(&FileConfig {
            database_url: Some("postgres://mem".into()),
            invite_only: Some(true),
            ..Default::default()
        });
        assert_eq!(r.get().database_url.as_deref(), Some("postgres://mem"));
        assert_eq!(r.get().invite_only, None);
        let file = FileConfig {
            database_url: Some("postgres://file".into()),
            ..Default::default()
        };
        let b = Bootstrap::from_file(&file, &|_| None, &r).unwrap();
        assert_eq!(b.database_url.as_deref(), Some("postgres://file"));
        let b = Bootstrap::from_file(&FileConfig::default(), &|_| None, &r).unwrap();
        assert_eq!(b.database_url.as_deref(), Some("postgres://mem"));
        assert!(!b.url_from_env);
        let env = |k: &str| (k == "DATABASE_URL").then(|| "postgres://env".to_string());
        let b = Bootstrap::from_file(&file, &env, &r).unwrap();
        assert_eq!(b.database_url.as_deref(), Some("postgres://env"));
        assert!(b.url_from_env);
    }
}
