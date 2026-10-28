//! All server configuration comes from environment variables. See
//! `.env.example` at the repo root and `docs/self-hosting.md` for the full
//! list and defaults.

use std::env;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;

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
}

#[derive(Clone)]
pub struct OidcConfig {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let database_url =
            env::var("DATABASE_URL").context("DATABASE_URL environment variable is required")?;
        let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8443".to_string());

        let key_b64 = env::var("SERVER_MASTER_KEY")
            .context("SERVER_MASTER_KEY environment variable is required (base64, 32 bytes)")?;
        let key_bytes = STANDARD
            .decode(key_b64.trim())
            .context("SERVER_MASTER_KEY is not valid base64")?;
        let server_master_key: [u8; 32] = key_bytes.try_into().map_err(|v: Vec<u8>| {
            anyhow!("SERVER_MASTER_KEY must decode to 32 bytes, got {}", v.len())
        })?;

        let signups_enabled = env_bool("SIGNUPS_ENABLED", true)?;
        let invite_only = env_bool("INVITE_ONLY", false)?;
        let max_devices_per_user = env_u32("MAX_DEVICES_PER_USER", 10)?;
        let rate_limit_per_min = env_u32("RATE_LIMIT_PER_MIN", 30)?;
        let session_ttl_hours = env_u32("SESSION_TTL_HOURS", 24 * 30)? as i64;

        let oidc = match (
            env::var("OIDC_ISSUER").ok(),
            env::var("OIDC_CLIENT_ID").ok(),
            env::var("OIDC_CLIENT_SECRET").ok(),
            env::var("OIDC_REDIRECT_URL").ok(),
        ) {
            (Some(issuer), Some(client_id), Some(client_secret), Some(redirect_url))
                if !issuer.is_empty() =>
            {
                Some(OidcConfig {
                    issuer,
                    client_id,
                    client_secret,
                    redirect_url,
                })
            }
            _ => None,
        };

        Ok(Self {
            database_url,
            bind_addr,
            server_master_key,
            signups_enabled,
            invite_only,
            max_devices_per_user,
            rate_limit_per_min,
            session_ttl_hours,
            oidc,
        })
    }

    pub fn session_ttl(&self) -> Duration {
        Duration::from_secs((self.session_ttl_hours.max(1) as u64) * 3600)
    }
}

fn env_bool(name: &str, default: bool) -> Result<bool> {
    match env::var(name) {
        Ok(v) => parse_bool(name, &v),
        Err(_) => Ok(default),
    }
}

fn parse_bool(name: &str, v: &str) -> Result<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => Err(anyhow!("{name} must be a boolean, got {other:?}")),
    }
}

fn env_u32(name: &str, default: u32) -> Result<u32> {
    match env::var(name) {
        Ok(v) => v
            .trim()
            .parse::<u32>()
            .with_context(|| format!("{name} must be a non-negative integer")),
        Err(_) => Ok(default),
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
}
