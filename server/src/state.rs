use std::sync::Arc;

use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use ourshell_proto::opaque::{opaque_ke, CipherSuite};

use crate::config::Config;
use crate::oidc::OidcClient;
use crate::rate_limit::RateLimiter;
use crate::sqlx;
use crate::sqlx::PgPool;

pub type ServerSetup = opaque_ke::ServerSetup<CipherSuite>;

const AAD_SERVER_SETUP: &[u8] = b"ourshell:server-setup:v1";

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

struct Inner {
    pool: PgPool,
    config: Config,
    server_setup: ServerSetup,
    rate_limiter: RateLimiter,
    oidc: Option<OidcClient>,
}

impl AppState {
    pub async fn new(pool: PgPool, config: Config) -> Result<Self> {
        let server_setup = load_or_create_server_setup(&pool, &config).await?;
        let rate_limiter =
            RateLimiter::new(config.rate_limit_per_min).with_proxy(config.proxy.clone());
        let oidc = match &config.oidc {
            Some(cfg) => Some(OidcClient::discover(cfg).await?),
            None => None,
        };
        Ok(Self(Arc::new(Inner {
            pool,
            config,
            server_setup,
            rate_limiter,
            oidc,
        })))
    }

    pub fn pool(&self) -> &PgPool {
        &self.0.pool
    }

    pub fn config(&self) -> &Config {
        &self.0.config
    }

    pub fn server_setup(&self) -> &ServerSetup {
        &self.0.server_setup
    }

    pub fn rate_limiter(&self) -> &RateLimiter {
        &self.0.rate_limiter
    }

    pub fn oidc(&self) -> Option<&OidcClient> {
        self.0.oidc.as_ref()
    }
}

async fn load_or_create_server_setup(pool: &PgPool, config: &Config) -> Result<ServerSetup> {
    if let Ok(b64) = std::env::var("OPAQUE_SERVER_SETUP") {
        if !b64.trim().is_empty() {
            let bytes = STANDARD
                .decode(b64.trim())
                .context("OPAQUE_SERVER_SETUP is not valid base64")?;
            return ServerSetup::deserialize(&bytes)
                .map_err(|e| anyhow::anyhow!("OPAQUE_SERVER_SETUP is malformed: {e}"));
        }
    }

    if let Some(row) =
        sqlx::query_as::<_, (Vec<u8>,)>("SELECT encrypted_setup FROM server_setup WHERE id = 1")
            .fetch_optional(pool)
            .await?
    {
        let plaintext = crate::crypto::decrypt(&config.server_master_key, &row.0, AAD_SERVER_SETUP)
            .map_err(|e| anyhow::anyhow!("failed to decrypt stored server_setup: {e}"))?;
        return ServerSetup::deserialize(&plaintext)
            .map_err(|e| anyhow::anyhow!("stored server_setup is malformed: {e}"));
    }

    let setup = ServerSetup::new(&mut rand::rngs::OsRng);
    let serialized = setup.serialize();
    let encrypted =
        crate::crypto::encrypt(&config.server_master_key, &serialized, AAD_SERVER_SETUP);
    let inserted = sqlx::query(
        "INSERT INTO server_setup (id, encrypted_setup) VALUES (1, $1) ON CONFLICT (id) DO NOTHING",
    )
    .bind(&encrypted)
    .execute(pool)
    .await?;

    if inserted.rows_affected() == 1 {
        return Ok(setup);
    }

    // Lost a startup race against another replica: load whatever won.
    let row: (Vec<u8>,) = sqlx::query_as("SELECT encrypted_setup FROM server_setup WHERE id = 1")
        .fetch_one(pool)
        .await?;
    let plaintext = crate::crypto::decrypt(&config.server_master_key, &row.0, AAD_SERVER_SETUP)
        .map_err(|e| anyhow::anyhow!("failed to decrypt stored server_setup: {e}"))?;
    ServerSetup::deserialize(&plaintext)
        .map_err(|e| anyhow::anyhow!("stored server_setup is malformed: {e}"))
}
