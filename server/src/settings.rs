//! The server's persistent configuration file, `$OURSHELL_DATA_DIR/config.toml`.
//!
//! The setup wizard and the admin page write it; environment variables
//! override it key by key, so existing env-only deployments (docker-compose
//! with a `.env`) keep working unchanged. A key that comes from the
//! environment is reported as *locked*: the admin page shows it read-only.

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
        "# OurShell server configuration. Written by the setup wizard and the\n\
         # /admin page; environment variables override any key here.\n\
         # Keep a copy of server_master_key: database backups can't be\n\
         # restored without it.\n\n{}",
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
];

/// The file config with environment overrides applied, plus which keys the
/// environment pinned (`"oidc"` stands for all four `OIDC_*` variables).
#[derive(Clone, Debug, Default)]
pub struct Effective {
    pub values: FileConfig,
    pub env_locked: Vec<&'static str>,
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
    })
}

/// [`read`] + [`apply_env`] with the real process environment.
pub fn load(dir: &Path) -> Result<Effective> {
    apply_env(&read(dir)?, &|k| std::env::var(k).ok())
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
    std::fs::create_dir_all(dir)?;
    std::fs::write(&path, &code).with_context(|| format!("failed to write {}", path.display()))?;
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
