//! Server configuration and where each key comes from.
//!
//! Precedence, highest first: environment variables > `config.toml` in the
//! data dir (`$OURSHELL_DATA_DIR`) > the encrypted settings row in the
//! database (see [`db`]) > built-in defaults.
//!
//! The *bootstrap* keys (`database_url`, `server_master_key`, `bind_addr`,
//! see [`BOOTSTRAP_KEYS`]) are needed before the database can be read, so
//! they only ever come from the environment or `config.toml`. Every other
//! setting is written by the setup wizard and the admin page into the
//! database, so a deployment without a persistent data volume keeps them
//! across redeploys. A key that comes from the environment (or from
//! `config.toml`, which also overrides the database) is reported as
//! *locked*: the admin page shows it read-only.

pub mod db;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const CONFIG_FILE: &str = "config.toml";
pub const SETUP_CODE_FILE: &str = "setup-code";

/// Where `config.toml` and other server state live: `$OURSHELL_DATA_DIR`,
/// `/data` in the Docker image, `./data` otherwise.
pub fn data_dir() -> PathBuf {
    std::env::var_os("OURSHELL_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("data"))
}

/// Every key is optional: a missing key means "use the environment, or the
/// built-in default".
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FileConfig {
    pub database_url: Option<String>,
    pub bind_addr: Option<String>,
    /// Base64, 32 bytes.
    pub server_master_key: Option<String>,
    pub public_url: Option<String>,
    pub signups_enabled: Option<bool>,
    pub invite_only: Option<bool>,
    pub max_devices_per_user: Option<u32>,
    pub rate_limit_per_min: Option<u32>,
    pub session_ttl_hours: Option<u32>,
    /// Argon2id hash for the `/admin` page (see `web::password`).
    pub admin_password_hash: Option<String>,
    pub oidc: Option<FileOidc>,
    pub tls: Option<FileTls>,
    /// A reverse proxy / ingress in front terminates HTTPS (see `crate::proxy`).
    pub behind_proxy: Option<bool>,
    /// Proxies whose `X-Forwarded-*` headers are believed (IPs or CIDRs);
    /// empty = loopback and private ranges.
    pub trusted_proxies: Option<Vec<String>>,
    /// Scheduled encrypted backups to S3-compatible storage (see `crate::backup`).
    pub backup: Option<FileBackup>,
}

/// `[backup]`: every key optional; defaults live in `crate::backup::BackupConfig`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FileBackup {
    pub enabled: Option<bool>,
    /// `https://s3.eu-central-1.amazonaws.com`, `https://<account>.r2.cloudflarestorage.com`,
    /// `http://nas:9000`, ... Empty = AWS S3 for `region`.
    pub endpoint: Option<String>,
    pub region: Option<String>,
    pub bucket: Option<String>,
    /// Key prefix ("folder") inside the bucket, e.g. `ourshell/`.
    pub prefix: Option<String>,
    pub access_key_id: Option<String>,
    pub secret_access_key: Option<String>,
    /// `https://endpoint/bucket/key` instead of `https://bucket.endpoint/key`.
    pub path_style: Option<bool>,
    /// Encrypts every backup file (Argon2id -> XChaCha20-Poly1305).
    pub passphrase: Option<String>,
    pub interval_hours: Option<u32>,
    /// How many backup files to keep in the bucket; 0 = all.
    pub keep: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FileOidc {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FileTls {
    /// Domain to request a Let's Encrypt certificate for. Empty = plain HTTP.
    pub domain: String,
    /// Contact email for the ACME account.
    pub email: Option<String>,
}

/// Keys that stay out of the database (they're needed to reach and decrypt
/// it): environment or `config.toml` only.
pub const BOOTSTRAP_KEYS: &[&str] = &["database_url", "server_master_key", "bind_addr"];

impl FileConfig {
    /// Only the bootstrap keys (all `config.toml` holds from now on).
    pub fn bootstrap(&self) -> FileConfig {
        FileConfig {
            database_url: self.database_url.clone(),
            bind_addr: self.bind_addr.clone(),
            server_master_key: self.server_master_key.clone(),
            ..Default::default()
        }
    }

    /// Everything but the bootstrap keys (what the database holds).
    pub fn without_bootstrap(&self) -> FileConfig {
        FileConfig {
            database_url: None,
            bind_addr: None,
            server_master_key: None,
            ..self.clone()
        }
    }

    /// Sets any non-bootstrap key.
    pub fn has_settings(&self) -> bool {
        self.without_bootstrap() != FileConfig::default()
    }
}

/// `over` on top of `base`, key by key. `[oidc]`, `[tls]` and
/// `trusted_proxies` are single values; `[backup]` merges field by field.
pub fn merge(base: &FileConfig, over: &FileConfig) -> FileConfig {
    fn pick<T: Clone>(over: &Option<T>, base: &Option<T>) -> Option<T> {
        over.clone().or_else(|| base.clone())
    }
    let backup = match (&base.backup, &over.backup) {
        (None, None) => None,
        (b, o) => {
            let d = FileBackup::default();
            let (b, o) = (b.as_ref().unwrap_or(&d), o.as_ref().unwrap_or(&d));
            Some(FileBackup {
                enabled: pick(&o.enabled, &b.enabled),
                endpoint: pick(&o.endpoint, &b.endpoint),
                region: pick(&o.region, &b.region),
                bucket: pick(&o.bucket, &b.bucket),
                prefix: pick(&o.prefix, &b.prefix),
                access_key_id: pick(&o.access_key_id, &b.access_key_id),
                secret_access_key: pick(&o.secret_access_key, &b.secret_access_key),
                path_style: pick(&o.path_style, &b.path_style),
                passphrase: pick(&o.passphrase, &b.passphrase),
                interval_hours: pick(&o.interval_hours, &b.interval_hours),
                keep: pick(&o.keep, &b.keep),
            })
        }
    };
    FileConfig {
        database_url: pick(&over.database_url, &base.database_url),
        bind_addr: pick(&over.bind_addr, &base.bind_addr),
        server_master_key: pick(&over.server_master_key, &base.server_master_key),
        public_url: pick(&over.public_url, &base.public_url),
        signups_enabled: pick(&over.signups_enabled, &base.signups_enabled),
        invite_only: pick(&over.invite_only, &base.invite_only),
        max_devices_per_user: pick(&over.max_devices_per_user, &base.max_devices_per_user),
        rate_limit_per_min: pick(&over.rate_limit_per_min, &base.rate_limit_per_min),
        session_ttl_hours: pick(&over.session_ttl_hours, &base.session_ttl_hours),
        admin_password_hash: pick(&over.admin_password_hash, &base.admin_password_hash),
        oidc: pick(&over.oidc, &base.oidc),
        tls: pick(&over.tls, &base.tls),
        behind_proxy: pick(&over.behind_proxy, &base.behind_proxy),
        trusted_proxies: pick(&over.trusted_proxies, &base.trusted_proxies),
        backup,
    }
}

/// The keys `cfg` sets, named like [`ENV_KEYS`] (plus `"oidc"` and
/// `"admin_password_hash"`), in [`ENV_KEYS`] order.
pub fn keys_set(cfg: &FileConfig) -> Vec<&'static str> {
    let b = cfg.backup.clone().unwrap_or_default();
    let tls = cfg.tls.as_ref();
    let mut out: Vec<&'static str> = ENV_KEYS
        .iter()
        .map(|(k, _)| *k)
        .filter(|k| match *k {
            "database_url" => cfg.database_url.is_some(),
            "bind_addr" => cfg.bind_addr.is_some(),
            "server_master_key" => cfg.server_master_key.is_some(),
            "public_url" => cfg.public_url.is_some(),
            "signups_enabled" => cfg.signups_enabled.is_some(),
            "invite_only" => cfg.invite_only.is_some(),
            "max_devices_per_user" => cfg.max_devices_per_user.is_some(),
            "rate_limit_per_min" => cfg.rate_limit_per_min.is_some(),
            "session_ttl_hours" => cfg.session_ttl_hours.is_some(),
            "tls_domain" => tls.is_some(),
            "tls_email" => tls.is_some_and(|t| t.email.is_some()),
            "behind_proxy" => cfg.behind_proxy.is_some(),
            "trusted_proxies" => cfg.trusted_proxies.is_some(),
            "backup_enabled" => b.enabled.is_some(),
            "backup_endpoint" => b.endpoint.is_some(),
            "backup_region" => b.region.is_some(),
            "backup_bucket" => b.bucket.is_some(),
            "backup_prefix" => b.prefix.is_some(),
            "backup_access_key_id" => b.access_key_id.is_some(),
            "backup_secret_access_key" => b.secret_access_key.is_some(),
            "backup_path_style" => b.path_style.is_some(),
            "backup_passphrase" => b.passphrase.is_some(),
            "backup_interval_hours" => b.interval_hours.is_some(),
            "backup_keep" => b.keep.is_some(),
            _ => false,
        })
        .collect();
    if cfg.oidc.is_some() {
        out.push("oidc");
    }
    if cfg.admin_password_hash.is_some() {
        out.push("admin_password_hash");
    }
    out
}

/// Reads `config.toml`; a missing file is an empty config.
pub fn read(dir: &Path) -> Result<FileConfig> {
    let path = dir.join(CONFIG_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).with_context(|| format!("{} is invalid", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(FileConfig::default()),
        Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// Serializes a config the way [`write`] stores it (also offered as the
/// setup wizard's backup download).
pub fn to_toml(cfg: &FileConfig) -> Result<String> {
    Ok(format!(
        "# OurShell server configuration. The setup wizard writes the database\n\
         # URL and master key here; all other settings live (encrypted) in the\n\
         # database and are changed on /admin. Environment variables override\n\
         # any key here, and keys here override the database.\n\
         # Keep a copy of server_master_key: the database can't be read without\n\
         # it.\n\n{}",
        toml::to_string_pretty(cfg)?
    ))
}

/// Writes `config.toml` atomically (temp file + rename), readable only by
/// the server's user.
pub fn write(dir: &Path, cfg: &FileConfig) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let path = dir.join(CONFIG_FILE);
    let tmp = dir.join(format!("{CONFIG_FILE}.tmp"));
    std::fs::write(&tmp, to_toml(cfg)?)
        .with_context(|| format!("failed to write {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, &path).with_context(|| format!("failed to replace {}", path.display()))
}

/// The environment variable behind each overridable key.
pub const ENV_KEYS: &[(&str, &str)] = &[
    ("database_url", "DATABASE_URL"),
    ("bind_addr", "BIND_ADDR"),
    ("server_master_key", "SERVER_MASTER_KEY"),
    ("public_url", "PUBLIC_URL"),
    ("signups_enabled", "SIGNUPS_ENABLED"),
    ("invite_only", "INVITE_ONLY"),
    ("max_devices_per_user", "MAX_DEVICES_PER_USER"),
    ("rate_limit_per_min", "RATE_LIMIT_PER_MIN"),
    ("session_ttl_hours", "SESSION_TTL_HOURS"),
    ("tls_domain", "OURSHELL_TLS_DOMAIN"),
    ("tls_email", "OURSHELL_TLS_EMAIL"),
    ("behind_proxy", "OURSHELL_BEHIND_PROXY"),
    ("trusted_proxies", "OURSHELL_TRUSTED_PROXIES"),
    ("backup_enabled", "OURSHELL_BACKUP_ENABLED"),
    ("backup_endpoint", "OURSHELL_BACKUP_S3_ENDPOINT"),
    ("backup_region", "OURSHELL_BACKUP_S3_REGION"),
    ("backup_bucket", "OURSHELL_BACKUP_S3_BUCKET"),
    ("backup_prefix", "OURSHELL_BACKUP_S3_PREFIX"),
    ("backup_access_key_id", "OURSHELL_BACKUP_S3_ACCESS_KEY_ID"),
    (
        "backup_secret_access_key",
        "OURSHELL_BACKUP_S3_SECRET_ACCESS_KEY",
    ),
    ("backup_path_style", "OURSHELL_BACKUP_S3_PATH_STYLE"),
    ("backup_passphrase", "OURSHELL_BACKUP_PASSPHRASE"),
    ("backup_interval_hours", "OURSHELL_BACKUP_INTERVAL_HOURS"),
    ("backup_keep", "OURSHELL_BACKUP_KEEP"),
];

/// Removes the value behind an [`ENV_KEYS`] key (or `"oidc"`) from `file`,
/// e.g. so a restored `config.toml` doesn't carry values the environment
/// sets anyway.
pub fn clear_key(file: &mut FileConfig, key: &str) {
    match key {
        "database_url" => file.database_url = None,
        "bind_addr" => file.bind_addr = None,
        "server_master_key" => file.server_master_key = None,
        "public_url" => file.public_url = None,
        "signups_enabled" => file.signups_enabled = None,
        "invite_only" => file.invite_only = None,
        "max_devices_per_user" => file.max_devices_per_user = None,
        "rate_limit_per_min" => file.rate_limit_per_min = None,
        "session_ttl_hours" => file.session_ttl_hours = None,
        "tls_domain" | "tls_email" => file.tls = None,
        "behind_proxy" => file.behind_proxy = None,
        "trusted_proxies" => file.trusted_proxies = None,
        "oidc" => file.oidc = None,
        k => {
            if let (Some(b), Some(field)) = (file.backup.as_mut(), k.strip_prefix("backup_")) {
                match field {
                    "enabled" => b.enabled = None,
                    "endpoint" => b.endpoint = None,
                    "region" => b.region = None,
                    "bucket" => b.bucket = None,
                    "prefix" => b.prefix = None,
                    "access_key_id" => b.access_key_id = None,
                    "secret_access_key" => b.secret_access_key = None,
                    "path_style" => b.path_style = None,
                    "passphrase" => b.passphrase = None,
                    "interval_hours" => b.interval_hours = None,
                    "keep" => b.keep = None,
                    _ => {}
                }
            }
        }
    }
}

/// The merged settings, plus which keys the environment pinned (`"oidc"`
/// stands for all four `OIDC_*` variables) and which non-bootstrap keys
/// `config.toml` sets (they override the database, so the admin page shows
/// them read-only too).
#[derive(Clone, Debug, Default)]
pub struct Effective {
    pub values: FileConfig,
    pub env_locked: Vec<&'static str>,
    pub file_locked: Vec<&'static str>,
}

impl Effective {
    /// Set by the environment or `config.toml`: not editable on `/admin`.
    pub fn locked(&self, key: &str) -> bool {
        self.env_locked.contains(&key) || self.file_locked.contains(&key)
    }
}

/// The full precedence chain: `env` > `file` (`config.toml`) > `db` (the
/// database's settings row; its bootstrap keys are ignored) > defaults.
pub fn resolve(
    file: &FileConfig,
    db: Option<&FileConfig>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Effective> {
    let base = db.map(FileConfig::without_bootstrap).unwrap_or_default();
    let mut eff = apply_env(&merge(&base, file), env)?;
    eff.file_locked = keys_set(&file.without_bootstrap());
    Ok(eff)
}

/// Overlays environment variables (read through `env`, so tests needn't
/// touch the process environment) onto `file`.
pub fn apply_env(file: &FileConfig, env: &dyn Fn(&str) -> Option<String>) -> Result<Effective> {
    let mut v = file.clone();
    let mut locked = Vec::new();
    for &(key, var) in ENV_KEYS {
        let Some(raw) = env(var).filter(|s| !s.trim().is_empty()) else {
            continue;
        };
        locked.push(key);
        let s = raw.trim().to_string();
        match key {
            "database_url" => v.database_url = Some(s),
            "bind_addr" => v.bind_addr = Some(s),
            "server_master_key" => v.server_master_key = Some(s),
            "public_url" => v.public_url = Some(s),
            "signups_enabled" => v.signups_enabled = Some(crate::config::parse_bool(var, &s)?),
            "invite_only" => v.invite_only = Some(crate::config::parse_bool(var, &s)?),
            "max_devices_per_user" => v.max_devices_per_user = Some(parse_u32(var, &s)?),
            "rate_limit_per_min" => v.rate_limit_per_min = Some(parse_u32(var, &s)?),
            "session_ttl_hours" => v.session_ttl_hours = Some(parse_u32(var, &s)?),
            "tls_domain" => v.tls.get_or_insert_with(Default::default).domain = s,
            "tls_email" => v.tls.get_or_insert_with(Default::default).email = Some(s),
            "behind_proxy" => v.behind_proxy = Some(crate::config::parse_bool(var, &s)?),
            "trusted_proxies" => v.trusted_proxies = Some(crate::proxy::split_list(&s)),
            k if k.starts_with("backup_") => {
                let b = v.backup.get_or_insert_with(Default::default);
                match &k["backup_".len()..] {
                    "enabled" => b.enabled = Some(crate::config::parse_bool(var, &s)?),
                    "endpoint" => b.endpoint = Some(s),
                    "region" => b.region = Some(s),
                    "bucket" => b.bucket = Some(s),
                    "prefix" => b.prefix = Some(s),
                    "access_key_id" => b.access_key_id = Some(s),
                    "secret_access_key" => b.secret_access_key = Some(s),
                    "path_style" => b.path_style = Some(crate::config::parse_bool(var, &s)?),
                    "passphrase" => b.passphrase = Some(s),
                    "interval_hours" => b.interval_hours = Some(parse_u32(var, &s)?),
                    "keep" => b.keep = Some(parse_u32(var, &s)?),
                    _ => unreachable!("every backup ENV_KEYS entry is handled"),
                }
            }
            _ => unreachable!("every ENV_KEYS entry is handled"),
        }
    }
    let oidc = (
        env("OIDC_ISSUER"),
        env("OIDC_CLIENT_ID"),
        env("OIDC_CLIENT_SECRET"),
        env("OIDC_REDIRECT_URL"),
    );
    if let (Some(issuer), Some(client_id), Some(client_secret), Some(redirect_url)) = oidc {
        if !issuer.trim().is_empty() {
            locked.push("oidc");
            v.oidc = Some(FileOidc {
                issuer,
                client_id,
                client_secret,
                redirect_url,
            });
        }
    }
    Ok(Effective {
        values: v,
        env_locked: locked,
        file_locked: Vec::new(),
    })
}

/// `config.toml` + the real process environment, without the database:
/// enough for the bootstrap keys (see [`Store::load`] for everything).
pub fn load(dir: &Path) -> Result<Effective> {
    resolve(&read(dir)?, None, &process_env)
}

/// The process environment, the way [`resolve`] takes it.
pub fn process_env(k: &str) -> Option<String> {
    std::env::var(k).ok()
}

/// The settings of a running (set-up) server: where to read and write them.
#[derive(Clone)]
pub struct Store {
    pub data_dir: PathBuf,
    pub pool: crate::sqlx::PgPool,
    pub master_key: [u8; 32],
    pub database_url: String,
}

impl Store {
    /// The database's settings (empty when there's no row yet).
    pub async fn read_db(&self) -> Result<FileConfig> {
        Ok(db::read(&self.pool, &self.master_key)
            .await?
            .unwrap_or_default())
    }

    /// Replaces the database's settings (bootstrap keys are dropped).
    pub async fn write_db(&self, cfg: &FileConfig) -> Result<()> {
        db::write(&self.pool, &self.master_key, cfg).await
    }

    /// Everything, with the precedence of [`resolve`]. The bootstrap keys
    /// this server runs with fill in what neither the environment nor
    /// `config.toml` has (e.g. remembered since the setup wizard).
    pub async fn load(&self) -> Result<Effective> {
        let db = self.read_db().await?;
        let mut eff = resolve(&read(&self.data_dir)?, Some(&db), &process_env)?;
        let v = &mut eff.values;
        if v.database_url.as_deref().is_none_or(str::is_empty) {
            v.database_url = Some(self.database_url.clone());
        }
        if v.server_master_key.as_deref().is_none_or(str::is_empty) {
            use base64::Engine;
            v.server_master_key =
                Some(base64::engine::general_purpose::STANDARD.encode(self.master_key));
        }
        Ok(eff)
    }
}

/// Suffix of the copy [`write_bootstrap`] keeps of a `config.toml` whose
/// settings moved into the database.
pub const LEGACY_COPY_SUFFIX: &str = "before-db-settings";

/// Sets the bootstrap keys `boot` has in `config.toml` (keeping the
/// file's others). With `strip_settings`, every non-bootstrap key is
/// removed from the file (they live in the database now); the original is
/// first copied to `config.toml.before-db-settings`, so nothing is lost.
/// Returns the text written.
pub fn update_bootstrap(dir: &Path, boot: &FileConfig, strip_settings: bool) -> Result<String> {
    let mut file = merge(&read(dir)?, &boot.bootstrap());
    if strip_settings && file.has_settings() {
        save_copy(dir, LEGACY_COPY_SUFFIX)?;
        file = file.bootstrap();
    }
    write(dir, &file)?;
    to_toml(&file)
}

/// Copies `config.toml` to `config.toml.<suffix>` (or, if that exists
/// already, `config.toml.<suffix>-<timestamp>`). Returns the copy's path.
pub fn save_copy(dir: &Path, suffix: &str) -> Result<PathBuf> {
    let path = dir.join(CONFIG_FILE);
    let mut copy = dir.join(format!("{CONFIG_FILE}.{suffix}"));
    if copy.exists() {
        copy = dir.join(format!(
            "{CONFIG_FILE}.{suffix}-{}",
            chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ")
        ));
    }
    std::fs::copy(&path, &copy).with_context(|| format!("failed to copy {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o600));
    }
    Ok(copy)
}

fn parse_u32(name: &str, v: &str) -> Result<u32> {
    v.parse::<u32>()
        .with_context(|| format!("{name} must be a non-negative integer"))
}

/// The one-time code that unlocks the setup wizard: `$OURSHELL_SETUP_CODE`
/// (the installer sets it so it can show the user), else a code stored in
/// the data dir, generated on first use.
pub fn setup_code(dir: &Path) -> Result<String> {
    if let Ok(code) = std::env::var("OURSHELL_SETUP_CODE") {
        if !code.trim().is_empty() {
            return Ok(code.trim().to_string());
        }
    }
    let path = dir.join(SETUP_CODE_FILE);
    if let Ok(code) = std::fs::read_to_string(&path) {
        if !code.trim().is_empty() {
            return Ok(code.trim().to_string());
        }
    }
    let code = generate_setup_code();
    // A read-only data dir: the code then lives only in memory.
    if let Err(e) = std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&path, &code)) {
        tracing::warn!(
            error = %e,
            "couldn't save the setup code in {}; it changes on every restart",
            dir.display()
        );
    }
    Ok(code)
}

/// `XXXX-XXXX-XXXX-XXXX` from an unambiguous alphabet (80 bits).
pub fn generate_setup_code() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 10];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let raw = base32::encode(base32::Alphabet::Crockford, &bytes);
    raw.as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("-")
}

/// Setup codes compare case-insensitively, ignoring dashes and spaces, in
/// constant time for equal-length inputs.
pub fn setup_code_matches(expected: &str, given: &str) -> bool {
    let norm = |s: &str| -> Vec<u8> {
        s.bytes()
            .filter(|b| !matches!(b, b'-' | b' '))
            .map(|b| b.to_ascii_uppercase())
            .collect()
    };
    let (a, b) = (norm(expected), norm(given));
    if a.is_empty() || a.len() != b.len() {
        return false;
    }
    a.iter().zip(&b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn env_overrides_file_and_is_locked() {
        let file = FileConfig {
            database_url: Some("postgres://file".into()),
            invite_only: Some(false),
            max_devices_per_user: Some(3),
            ..Default::default()
        };
        let eff = apply_env(
            &file,
            &env_of(&[("DATABASE_URL", "postgres://env"), ("INVITE_ONLY", "yes")]),
        )
        .unwrap();
        assert_eq!(eff.values.database_url.as_deref(), Some("postgres://env"));
        assert_eq!(eff.values.invite_only, Some(true));
        assert_eq!(eff.values.max_devices_per_user, Some(3));
        assert_eq!(eff.env_locked, vec!["database_url", "invite_only"]);
    }

    #[test]
    fn proxy_env_vars() {
        let eff = apply_env(
            &FileConfig::default(),
            &env_of(&[
                ("OURSHELL_BEHIND_PROXY", "true"),
                ("OURSHELL_TRUSTED_PROXIES", "10.42.0.0/16, 203.0.113.5"),
            ]),
        )
        .unwrap();
        assert_eq!(eff.values.behind_proxy, Some(true));
        assert_eq!(
            eff.values.trusted_proxies,
            Some(vec!["10.42.0.0/16".into(), "203.0.113.5".into()])
        );
        assert!(eff.env_locked.contains(&"behind_proxy"));
        assert!(apply_env(
            &FileConfig::default(),
            &env_of(&[("OURSHELL_BEHIND_PROXY", "maybe")])
        )
        .is_err());
    }

    #[test]
    fn backup_env_vars_override_the_backup_table() {
        let file = FileConfig {
            backup: Some(FileBackup {
                bucket: Some("file-bucket".into()),
                keep: Some(5),
                passphrase: Some("from the file".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let eff = apply_env(
            &file,
            &env_of(&[
                ("OURSHELL_BACKUP_ENABLED", "yes"),
                ("OURSHELL_BACKUP_S3_BUCKET", " env-bucket "),
                ("OURSHELL_BACKUP_S3_PATH_STYLE", "1"),
                ("OURSHELL_BACKUP_INTERVAL_HOURS", "6"),
                ("OURSHELL_BACKUP_S3_SECRET_ACCESS_KEY", "s3cret"),
            ]),
        )
        .unwrap();
        let b = eff.values.backup.clone().unwrap();
        assert_eq!(b.enabled, Some(true));
        assert_eq!(b.bucket.as_deref(), Some("env-bucket"));
        assert_eq!(b.path_style, Some(true));
        assert_eq!(b.interval_hours, Some(6));
        assert_eq!(b.keep, Some(5));
        assert_eq!(b.passphrase.as_deref(), Some("from the file"));
        assert_eq!(b.secret_access_key.as_deref(), Some("s3cret"));
        for k in [
            "backup_enabled",
            "backup_bucket",
            "backup_path_style",
            "backup_interval_hours",
            "backup_secret_access_key",
        ] {
            assert!(eff.env_locked.contains(&k), "{k}");
        }
        assert!(!eff.env_locked.contains(&"backup_keep"));

        let mut cleared = eff.values.clone();
        for k in &eff.env_locked {
            clear_key(&mut cleared, k);
        }
        let b = cleared.backup.unwrap();
        assert_eq!((b.bucket, b.enabled, b.keep), (None, None, Some(5)));

        for bad in [
            ("OURSHELL_BACKUP_KEEP", "-1"),
            ("OURSHELL_BACKUP_S3_PATH_STYLE", "sometimes"),
        ] {
            assert!(apply_env(&FileConfig::default(), &env_of(&[bad])).is_err());
        }
    }

    #[test]
    fn bad_env_value_is_an_error() {
        let eff = apply_env(
            &FileConfig::default(),
            &env_of(&[("MAX_DEVICES_PER_USER", "lots")]),
        );
        assert!(eff.is_err());
    }

    #[test]
    fn oidc_needs_all_four_vars() {
        let partial = apply_env(
            &FileConfig::default(),
            &env_of(&[("OIDC_ISSUER", "https://i")]),
        )
        .unwrap();
        assert!(partial.values.oidc.is_none());
        let full = apply_env(
            &FileConfig::default(),
            &env_of(&[
                ("OIDC_ISSUER", "https://i"),
                ("OIDC_CLIENT_ID", "c"),
                ("OIDC_CLIENT_SECRET", "s"),
                ("OIDC_REDIRECT_URL", "https://r"),
            ]),
        )
        .unwrap();
        assert!(full.env_locked.contains(&"oidc"));
        assert_eq!(full.values.oidc.unwrap().client_id, "c");
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = std::env::temp_dir().join(format!("ourshell-settings-{}", uuid::Uuid::new_v4()));
        let cfg = FileConfig {
            database_url: Some("postgres://x".into()),
            signups_enabled: Some(false),
            tls: Some(FileTls {
                domain: "sync.example.com".into(),
                email: None,
            }),
            ..Default::default()
        };
        write(&dir, &cfg).unwrap();
        assert_eq!(read(&dir).unwrap(), cfg);
        assert_eq!(read(&dir.join("missing")).unwrap(), FileConfig::default());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn setup_codes() {
        let code = generate_setup_code();
        assert_eq!(code.len(), 19);
        assert!(setup_code_matches(
            &code,
            &code.to_lowercase().replace('-', " ")
        ));
        assert!(!setup_code_matches(&code, "AAAA-BBBB-CCCC-DDDD"));
        assert!(!setup_code_matches(&code, ""));
        assert!(!setup_code_matches("", ""));
    }
}
