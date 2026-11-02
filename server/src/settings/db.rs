//! The settings row in the database (`server_settings`, migration 0003):
//! every non-bootstrap key of [`FileConfig`] as JSON, encrypted with the
//! server master key so a database dump doesn't leak the admin password
//! hash, the OIDC client secret, the S3 secret key or the backup
//! passphrase. The master key itself is never stored in the database.

use anyhow::{Context, Result};
use thiserror::Error;

use super::FileConfig;
use crate::crypto;
use crate::sqlx::{self, PgPool};

/// AAD binding the ciphertext to its purpose.
pub const AAD: &[u8] = b"ourshell:server-settings:v1";

#[derive(Debug, Error)]
pub enum OpenError {
    #[error("the server master key doesn't decrypt the stored settings")]
    WrongKey,
    #[error("the stored settings are malformed: {0}")]
    Malformed(String),
}

/// Encrypts the non-bootstrap part of `cfg`.
pub fn seal(master_key: &[u8; 32], cfg: &FileConfig) -> Result<Vec<u8>> {
    let json = serde_json::to_vec(&cfg.without_bootstrap())?;
    Ok(crypto::encrypt(master_key, &json, AAD))
}

/// Decrypts what [`seal`] produced.
pub fn open(master_key: &[u8; 32], blob: &[u8]) -> Result<FileConfig, OpenError> {
    let json = crypto::decrypt(master_key, blob, AAD).map_err(|_| OpenError::WrongKey)?;
    let cfg: FileConfig =
        serde_json::from_slice(&json).map_err(|e| OpenError::Malformed(e.to_string()))?;
    Ok(cfg.without_bootstrap())
}

/// Whether the `server_settings` table exists (migration 0003 has run).
pub async fn table_exists(pool: &PgPool) -> Result<bool> {
    let (exists,): (bool,) = sqlx::query_as("SELECT to_regclass('server_settings') IS NOT NULL")
        .fetch_one(pool)
        .await?;
    Ok(exists)
}

/// The encrypted row, if the table and the row exist.
pub async fn read_raw(pool: &PgPool) -> Result<Option<Vec<u8>>> {
    if !table_exists(pool).await? {
        return Ok(None);
    }
    let row: Option<(Vec<u8>,)> =
        sqlx::query_as("SELECT encrypted_settings FROM server_settings WHERE id = 1")
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(b,)| b))
}

/// The stored settings; `None` without a row. A wrong key is an error.
pub async fn read(pool: &PgPool, master_key: &[u8; 32]) -> Result<Option<FileConfig>> {
    match read_raw(pool).await? {
        None => Ok(None),
        Some(blob) => Ok(Some(open(master_key, &blob)?)),
    }
}

/// Creates or replaces the row.
pub async fn write(pool: &PgPool, master_key: &[u8; 32], cfg: &FileConfig) -> Result<()> {
    let blob = seal(master_key, cfg)?;
    sqlx::query(
        "INSERT INTO server_settings (id, encrypted_settings, updated_at) VALUES (1, $1, now()) \
         ON CONFLICT (id) DO UPDATE SET encrypted_settings = EXCLUDED.encrypted_settings, \
         updated_at = now()",
    )
    .bind(&blob)
    .execute(pool)
    .await
    .context("failed to save the settings in the database")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{FileBackup, FileOidc};

    #[test]
    fn sealed_settings_round_trip_and_hide_secrets() {
        let key = [3u8; 32];
        let cfg = FileConfig {
            database_url: Some("postgres://never-stored".into()),
            server_master_key: Some("never stored either".into()),
            bind_addr: Some("0.0.0.0:1".into()),
            invite_only: Some(true),
            admin_password_hash: Some("$argon2id$secret-hash".into()),
            oidc: Some(FileOidc {
                issuer: "https://idp".into(),
                client_id: "c".into(),
                client_secret: "oidc-secret".into(),
                redirect_url: "https://r".into(),
            }),
            backup: Some(FileBackup {
                passphrase: Some("backup passphrase".into()),
                secret_access_key: Some("s3-secret".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let blob = seal(&key, &cfg).unwrap();
        for secret in [
            "secret-hash",
            "oidc-secret",
            "backup passphrase",
            "s3-secret",
            "never-stored",
            "never stored either",
        ] {
            assert!(
                !blob.windows(secret.len()).any(|w| w == secret.as_bytes()),
                "{secret} leaked"
            );
        }
        let back = open(&key, &blob).unwrap();
        assert_eq!(back, cfg.without_bootstrap());
        assert_eq!(back.database_url, None);
        assert_eq!(back.server_master_key, None);
        assert!(matches!(open(&[4u8; 32], &blob), Err(OpenError::WrongKey)));
        // Bound to its purpose: the same key with another AAD fails.
        let other = crypto::encrypt(&key, b"{}", b"ourshell:server-setup:v1");
        assert!(matches!(open(&key, &other), Err(OpenError::WrongKey)));
    }
}
