//! Server configuration: `config.toml` in the data dir (written by the setup
//! wizard and the admin page; see `crate::settings`) with environment
//! variables overriding it key by key. See `.env.example` at the repo root
//! and `docs/self-hosting.md` for the full list and defaults.

use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;

use crate::settings::{self, Effective};

pub const DEFAULT_BIND_ADDR: &str = "0.0.0.0:8443";

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub bind_addr: String,
    /// 32 raw bytes, decoded from `SERVER_MASTER_KEY` (base64).
    pub server_master_key: [u8; 32],
    pub signups_enabled: bool,
    pub invite_only: bool,
    pub max_devices_per_user: u32,
    pub rate_limit_per_min: u32,
    pub session_ttl_hours: i64,
    pub oidc: Option<OidcConfig>,
    /// The address clients use to reach this server, e.g. `https://sync.example.com`.
    pub public_url: Option<String>,
    /// Serve HTTPS with a Let's Encrypt certificate for this domain.
    pub tls: Option<TlsConfig>,
    /// Argon2id hash guarding the `/admin` page; `None` disables it.
    pub admin_password_hash: Option<String>,
}

#[derive(Clone)]
pub struct OidcConfig {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TlsConfig {
    pub domain: String,
    pub email: Option<String>,
}

impl Config {
    /// Environment variables only (no config file), as before the setup
    /// wizard existed. Fails if the database URL or master key is missing.
    pub fn from_env() -> Result<Self> {
        let eff = settings::apply_env(&Default::default(), &|k| std::env::var(k).ok())?;
        Self::from_effective(&eff)?.ok_or_else(|| {
            anyhow!("DATABASE_URL and SERVER_MASTER_KEY (base64, 32 bytes) are required")
        })
    }

    /// `config.toml` in `data_dir`, overridden by the environment. `None`
    /// means the server isn't set up yet (no database URL or master key).
    pub fn load(data_dir: &Path) -> Result<Option<Self>> {
        Self::from_effective(&settings::load(data_dir)?)
    }

    /// Builds a config from merged settings; `None` if the database URL or
    /// master key is missing.
    pub fn from_effective(eff: &Effective) -> Result<Option<Self>> {
        let v = &eff.values;
        let (Some(database_url), Some(key_b64)) = (
            v.database_url.clone().filter(|s| !s.is_empty()),
            v.server_master_key.as_deref().filter(|s| !s.is_empty()),
        ) else {
            return Ok(None);
        };
        let server_master_key = decode_master_key(key_b64)?;
        let oidc = v
            .oidc
            .clone()
            .filter(|o| !o.issuer.is_empty())
            .map(|o| OidcConfig {
                issuer: o.issuer,
                client_id: o.client_id,
                client_secret: o.client_secret,
                redirect_url: o.redirect_url,
            });
        let tls = v
            .tls
            .clone()
            .filter(|t| !t.domain.is_empty())
            .map(|t| TlsConfig {
                domain: t.domain,
                email: t.email.filter(|e| !e.is_empty()),
            });
        Ok(Some(Self {
            database_url,
            bind_addr: v
                .bind_addr
                .clone()
                .unwrap_or_else(|| DEFAULT_BIND_ADDR.to_string()),
            server_master_key,
            signups_enabled: v.signups_enabled.unwrap_or(true),
            invite_only: v.invite_only.unwrap_or(false),
            max_devices_per_user: v.max_devices_per_user.unwrap_or(10),
            rate_limit_per_min: v.rate_limit_per_min.unwrap_or(30),
            session_ttl_hours: v.session_ttl_hours.unwrap_or(24 * 30) as i64,
            oidc,
            public_url: v.public_url.clone().filter(|s| !s.is_empty()),
            tls,
            admin_password_hash: v.admin_password_hash.clone().filter(|s| !s.is_empty()),
        }))
    }

    pub fn session_ttl(&self) -> Duration {
        Duration::from_secs((self.session_ttl_hours.max(1) as u64) * 3600)
    }
}

pub fn decode_master_key(b64: &str) -> Result<[u8; 32]> {
    let bytes = STANDARD
        .decode(b64.trim())
        .context("SERVER_MASTER_KEY is not valid base64")?;
    bytes
        .try_into()
        .map_err(|v: Vec<u8>| anyhow!("SERVER_MASTER_KEY must decode to 32 bytes, got {}", v.len()))
}

/// A fresh random master key, base64.
pub fn generate_master_key() -> String {
    use rand::RngCore;
    let mut key = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut key);
    STANDARD.encode(key)
}

pub fn parse_bool(name: &str, v: &str) -> Result<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => Err(anyhow!("{name} must be a boolean, got {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bool_parses_common_forms() {
        assert!(parse_bool("x", "yes").unwrap());
        assert!(parse_bool("x", "TRUE").unwrap());
        assert!(!parse_bool("x", "0").unwrap());
        assert!(!parse_bool("x", "off").unwrap());
        assert!(parse_bool("x", "maybe").is_err());
    }

    #[test]
    fn incomplete_until_database_and_key() {
        let mut eff = Effective::default();
        assert!(Config::from_effective(&eff).unwrap().is_none());
        eff.values.database_url = Some("postgres://x".into());
        assert!(Config::from_effective(&eff).unwrap().is_none());
        eff.values.server_master_key = Some(generate_master_key());
        let cfg = Config::from_effective(&eff).unwrap().unwrap();
        assert_eq!(cfg.bind_addr, DEFAULT_BIND_ADDR);
        assert!(cfg.signups_enabled && !cfg.invite_only);
        eff.values.server_master_key = Some("c2hvcnQ=".into());
        assert!(Config::from_effective(&eff).is_err());
    }
}
