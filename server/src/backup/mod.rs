//! Built-in, encrypted, scheduled backups to S3-compatible storage.
//!
//! A backup is one `.osbk` file (see [`format`]): every application table
//! (Postgres `COPY`, see [`dump`]; this includes the encrypted
//! `server_settings` row), the effective configuration as TOML (the
//! master key, the database URL and every setting from the environment,
//! `config.toml` and the database, so one file restores everything) and a
//! manifest,
//! gzip-compressed and encrypted under a key derived from the backup
//! passphrase. A background task checks every `interval_hours`; when the
//! content hash (see [`format::Bundle::content_hash`]) hasn't changed since
//! the last upload, nothing is uploaded. State lives in
//! `<data_dir>/backup-state.json`. Restore with `ourshell-server restore`
//! (see [`restore`]).

pub mod dump;
pub mod format;
pub mod restore;
pub mod s3;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use self::format::{Bundle, KdfParams, Manifest, PAYLOAD_FORMAT};
use self::s3::{Object, S3Client, S3Config};
use crate::settings::{self, FileBackup, Store};
use crate::sqlx::PgPool;

pub const STATE_FILE: &str = "backup-state.json";
pub const MIN_PASSPHRASE_LEN: usize = 12;
pub const DEFAULT_INTERVAL_HOURS: u32 = 24;
pub const DEFAULT_KEEP: u32 = 30;
pub const DEFAULT_REGION: &str = "us-east-1";
/// The first scheduled check after (re)start.
pub const FIRST_CHECK_DELAY: Duration = Duration::from_secs(120);

/// `[backup]` with defaults applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupConfig {
    pub enabled: bool,
    pub s3: S3Config,
    /// Normalized: empty, or ends with `/`.
    pub prefix: String,
    pub passphrase: String,
    pub interval_hours: u32,
    /// 0 = keep everything.
    pub keep: u32,
}

impl BackupConfig {
    pub fn from_file(b: Option<&FileBackup>) -> Self {
        let d = FileBackup::default();
        let b = b.unwrap_or(&d);
        let text = |v: &Option<String>| v.as_deref().map(str::trim).unwrap_or_default().to_string();
        Self {
            enabled: b.enabled.unwrap_or(false),
            s3: S3Config {
                endpoint: text(&b.endpoint),
                region: Some(text(&b.region))
                    .filter(|r| !r.is_empty())
                    .unwrap_or_else(|| DEFAULT_REGION.to_string()),
                bucket: text(&b.bucket),
                access_key_id: text(&b.access_key_id),
                secret_access_key: b.secret_access_key.clone().unwrap_or_default(),
                path_style: b.path_style.unwrap_or(false),
            },
            prefix: normalize_prefix(&text(&b.prefix)),
            passphrase: b.passphrase.clone().unwrap_or_default(),
            interval_hours: b
                .interval_hours
                .filter(|h| *h > 0)
                .unwrap_or(DEFAULT_INTERVAL_HOURS),
            keep: b.keep.unwrap_or(DEFAULT_KEEP),
        }
    }

    /// The storage settings are complete enough to connect.
    pub fn check_storage(&self) -> Result<()> {
        if self.s3.bucket.is_empty() {
            bail!("set the bucket name");
        }
        if self.s3.access_key_id.is_empty() || self.s3.secret_access_key.is_empty() {
            bail!("set the access key ID and secret access key");
        }
        Ok(())
    }

    /// Everything needed to make a backup.
    pub fn check(&self) -> Result<()> {
        self.check_storage()?;
        if self.passphrase.chars().count() < MIN_PASSPHRASE_LEN {
            bail!("the backup passphrase needs at least {MIN_PASSPHRASE_LEN} characters");
        }
        Ok(())
    }

    pub fn client(&self) -> Result<S3Client> {
        self.check_storage()?;
        S3Client::new(self.s3.clone())
    }

    /// Effective settings: environment > `config.toml` > database.
    pub async fn load(store: &Store) -> Result<Self> {
        Ok(Self::from_file(store.load().await?.values.backup.as_ref()))
    }
}

/// `ourshell` -> `ourshell/`; leading slashes dropped.
pub fn normalize_prefix(p: &str) -> String {
    let p = p.trim().trim_start_matches('/');
    if p.is_empty() || p.ends_with('/') {
        p.to_string()
    } else {
        format!("{p}/")
    }
}

/// Backup files under `prefix` beyond the newest `keep` (0 = keep all),
/// oldest last. Other objects under the prefix are never touched.
pub fn select_for_deletion(objects: &[Object], prefix: &str, keep: u32) -> Vec<String> {
    if keep == 0 {
        return Vec::new();
    }
    let mut ours: Vec<&str> = objects
        .iter()
        .map(|o| o.key.as_str())
        .filter(|k| k.strip_prefix(prefix).is_some_and(format::is_backup_name))
        .collect();
    // The UTC timestamp in the name sorts chronologically.
    ours.sort_unstable_by(|a, b| b.cmp(a));
    ours.into_iter()
        .skip(keep as usize)
        .map(str::to_string)
        .collect()
}

/// `<data_dir>/backup-state.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackupState {
    /// Content hash of the last uploaded backup, hex.
    pub last_hash: Option<String>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_size: Option<u64>,
    pub last_key: Option<String>,
    pub last_check_at: Option<DateTime<Utc>>,
    /// `uploaded` | `unchanged` | `failed`.
    pub last_result: Option<String>,
    pub last_error: Option<String>,
    pub last_error_at: Option<DateTime<Utc>>,
}

impl BackupState {
    pub fn read(data_dir: &Path) -> Self {
        std::fs::read(data_dir.join(STATE_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn write(&self, data_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(data_dir)?;
        let tmp = data_dir.join(format!("{STATE_FILE}.tmp"));
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, data_dir.join(STATE_FILE))?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// The background schedule: skipped when backups are off or nothing changed.
    Scheduled,
    /// "Back up now": always uploads.
    Manual,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Outcome {
    Uploaded {
        key: String,
        size: u64,
        /// Old backups removed by retention.
        deleted: Vec<String>,
    },
    /// Nothing changed since `key` was uploaded.
    Unchanged { key: String },
    /// Scheduled run with backups turned off.
    Disabled,
    /// Another backup is in progress.
    Busy,
}

/// Backups for this process, shared by every serve generation (so two
/// never run at once, even across a reload).
pub struct Backups {
    data_dir: PathBuf,
    lock: tokio::sync::Mutex<()>,
    next_check: Mutex<Option<DateTime<Utc>>>,
    kdf: KdfParams,
}

impl Backups {
    pub fn new(data_dir: PathBuf) -> Arc<Self> {
        Self::with_kdf(data_dir, KdfParams::default())
    }

    /// Tests use a cheap key derivation.
    pub fn with_kdf(data_dir: PathBuf, kdf: KdfParams) -> Arc<Self> {
        Arc::new(Self {
            data_dir,
            lock: tokio::sync::Mutex::new(()),
            next_check: Mutex::new(None),
            kdf,
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn state(&self) -> BackupState {
        BackupState::read(&self.data_dir)
    }

    pub fn running(&self) -> bool {
        self.lock.try_lock().is_err()
    }

    pub fn next_check(&self) -> Option<DateTime<Utc>> {
        *self.next_check.lock().unwrap()
    }

    /// Builds the bundle for the database behind `store` and the current
    /// settings (without encrypting it).
    pub async fn snapshot(&self, store: &Store) -> Result<Bundle> {
        let eff = store.load().await?;
        snapshot(&store.pool, &settings::to_toml(&eff.values)?).await
    }

    /// One backup run. Errors are also recorded in the state file.
    pub async fn run(&self, store: &Store, trigger: Trigger) -> Result<Outcome> {
        let Ok(_guard) = self.lock.try_lock() else {
            return Ok(Outcome::Busy);
        };
        let cfg = BackupConfig::load(store).await?;
        if trigger == Trigger::Scheduled && !cfg.enabled {
            return Ok(Outcome::Disabled);
        }
        let result = self.run_locked(store, &cfg, trigger).await;
        let mut st = self.state();
        let now = Utc::now();
        st.last_check_at = Some(now);
        match &result {
            Ok(Outcome::Uploaded { key, size, .. }) => {
                st.last_result = Some("uploaded".into());
                st.last_success_at = Some(now);
                st.last_key = Some(key.clone());
                st.last_size = Some(*size);
                st.last_error = None;
            }
            Ok(Outcome::Unchanged { .. }) => {
                st.last_result = Some("unchanged".into());
                st.last_error = None;
            }
            Ok(_) => {}
            Err(e) => {
                st.last_result = Some("failed".into());
                st.last_error = Some(format!("{e:#}"));
                st.last_error_at = Some(now);
            }
        }
        if let Err(e) = st.write(&self.data_dir) {
            tracing::warn!(error = %e, "failed to write {STATE_FILE}");
        }
        result
    }

    async fn run_locked(
        &self,
        store: &Store,
        cfg: &BackupConfig,
        trigger: Trigger,
    ) -> Result<Outcome> {
        cfg.check()?;
        let client = cfg.client()?;
        let bundle = self.snapshot(store).await?;
        let hash = bundle.manifest.content_hash.clone();
        let prev = self.state();
        if trigger == Trigger::Scheduled && prev.last_hash.as_deref() == Some(hash.as_str()) {
            if let Some(key) = prev.last_key.clone() {
                // Unless someone deleted it from the bucket meanwhile.
                if client.object_exists(&key).await? {
                    return Ok(Outcome::Unchanged { key });
                }
            }
        }
        let key = format!(
            "{}{}",
            cfg.prefix,
            format::file_name(bundle.manifest.created_at)
        );
        let passphrase = cfg.passphrase.clone();
        let kdf = self.kdf;
        let file = tokio::task::spawn_blocking(move || format::seal(&bundle, &passphrase, kdf))
            .await
            .map_err(|e| anyhow!("encryption task failed: {e}"))??;
        let size = file.len() as u64;
        client
            .put_object(&key, file)
            .await
            .with_context(|| format!("uploading {key}"))?;
        // Record the upload before retention, so a failed cleanup doesn't
        // cause a re-upload of the same content.
        let mut st = self.state();
        st.last_hash = Some(hash);
        st.last_key = Some(key.clone());
        st.last_size = Some(size);
        st.last_success_at = Some(Utc::now());
        st.write(&self.data_dir)?;

        let mut deleted = Vec::new();
        if cfg.keep > 0 {
            let objects = client
                .list(&cfg.prefix)
                .await
                .context("listing old backups")?;
            for old in select_for_deletion(&objects, &cfg.prefix, cfg.keep) {
                if old == key {
                    continue;
                }
                client
                    .delete_object(&old)
                    .await
                    .with_context(|| format!("deleting old backup {old}"))?;
                deleted.push(old);
            }
        }
        tracing::info!(%key, size, deleted = deleted.len(), "backup uploaded");
        Ok(Outcome::Uploaded { key, size, deleted })
    }

    /// Starts the schedule for one serve generation; dropping the returned
    /// handle stops it (on reload, like the TLS tasks).
    pub fn start(self: &Arc<Self>, store: Store) -> Schedule {
        let me = self.clone();
        let task = tokio::spawn(async move {
            let mut next = Utc::now() + FIRST_CHECK_DELAY;
            loop {
                *me.next_check.lock().unwrap() = Some(next);
                let wait = (next - Utc::now()).to_std().unwrap_or_default();
                tokio::time::sleep(wait).await;
                match me.run(&store, Trigger::Scheduled).await {
                    Ok(Outcome::Unchanged { .. }) => {
                        tracing::info!("backup check: nothing changed, skipped")
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %format!("{e:#}"), "scheduled backup failed"),
                }
                let hours = BackupConfig::load(&store)
                    .await
                    .map(|c| c.interval_hours)
                    .unwrap_or(DEFAULT_INTERVAL_HOURS);
                next = Utc::now() + chrono::Duration::hours(i64::from(hours));
            }
        });
        Schedule(task)
    }
}

/// The running schedule; aborted on drop.
pub struct Schedule(tokio::task::JoinHandle<()>);

impl Drop for Schedule {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Exports the database behind `pool` into an unencrypted bundle, with
/// `config_toml` as its configuration.
pub async fn snapshot(pool: &PgPool, config_toml: &str) -> Result<Bundle> {
    let export = dump::export(pool).await?;
    let hash = format::content_hash(&export.migrations, config_toml, &export.tables);
    Ok(Bundle {
        manifest: Manifest {
            format: PAYLOAD_FORMAT,
            server_version: crate::web::VERSION.to_string(),
            created_at: Utc::now(),
            migrations: export.migrations,
            tables: export.info,
            content_hash: format::hex(&hash),
        },
        config_toml: config_toml.to_string(),
        tables: export.tables,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(key: &str) -> Object {
        Object {
            key: key.into(),
            size: 1,
            last_modified: None,
        }
    }

    #[test]
    fn retention_keeps_the_newest() {
        let objects = vec![
            obj("p/ourshell-backup-20261101T000000Z.osbk"),
            obj("p/ourshell-backup-20261103T000000Z.osbk"),
            obj("p/ourshell-backup-20261102T000000Z.osbk"),
            obj("p/notes.txt"),
            obj("p/sub/ourshell-backup-20200101T000000Z.osbk"),
            obj("other/ourshell-backup-20200101T000000Z.osbk"),
        ];
        assert_eq!(
            select_for_deletion(&objects, "p/", 1),
            vec![
                "p/ourshell-backup-20261102T000000Z.osbk",
                "p/ourshell-backup-20261101T000000Z.osbk"
            ]
        );
        assert_eq!(
            select_for_deletion(&objects, "p/", 2),
            vec!["p/ourshell-backup-20261101T000000Z.osbk"]
        );
        assert!(select_for_deletion(&objects, "p/", 3).is_empty());
        assert!(select_for_deletion(&objects, "p/", 0).is_empty());
        // No prefix: only top-level backup files count.
        assert!(select_for_deletion(&objects, "", 1).is_empty());
    }

    #[test]
    fn config_defaults_and_checks() {
        let c = BackupConfig::from_file(None);
        assert!(!c.enabled);
        assert_eq!(c.s3.region, "us-east-1");
        assert_eq!((c.interval_hours, c.keep), (24, 30));
        assert!(c.check().is_err());
        let mut fb = FileBackup {
            enabled: Some(true),
            bucket: Some("b".into()),
            access_key_id: Some("a".into()),
            secret_access_key: Some("s".into()),
            prefix: Some("/team/ourshell".into()),
            interval_hours: Some(0),
            passphrase: Some("short".into()),
            ..Default::default()
        };
        let c = BackupConfig::from_file(Some(&fb));
        assert_eq!(c.prefix, "team/ourshell/");
        assert_eq!(c.interval_hours, 24);
        assert!(c.check_storage().is_ok());
        assert!(c.check().unwrap_err().to_string().contains("12"));
        fb.passphrase = Some("twelve chars".into());
        assert!(BackupConfig::from_file(Some(&fb)).check().is_ok());
        assert_eq!(normalize_prefix(""), "");
        assert_eq!(normalize_prefix("a/"), "a/");
    }
}
