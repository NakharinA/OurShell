//! `ourshell-server restore <file.osbk | s3:KEY | s3:latest> [--dry-run]`.

use anyhow::{anyhow, bail, ensure, Context, Result};

use super::format::{self, Bundle};
use super::{dump, BackupConfig};
use crate::settings::{self, Effective, FileConfig};
use crate::sqlx::migrate::Migrator;
use crate::sqlx::{PgPool, PgPoolOptions};

const USAGE: &str = "usage: ourshell-server restore <file.osbk | s3:KEY | s3:latest> [--dry-run]";

/// Refuses a backup made by a server with migrations this one doesn't have.
pub fn check_migrations(backup: &[i64], known: &[i64]) -> Result<()> {
    let unknown: Vec<i64> = backup
        .iter()
        .copied()
        .filter(|v| !known.contains(v))
        .collect();
    ensure!(
        unknown.is_empty(),
        "this backup was made by a newer OurShell server (database migrations {unknown:?} are unknown here); \
         upgrade this server first, then restore"
    );
    Ok(())
}

pub fn known_migrations(migrator: &Migrator) -> Vec<i64> {
    migrator.iter().map(|m| m.version).collect()
}

/// Migrates the database behind `pool` to this server's version, then
/// replaces all its application data with the backup's in one transaction.
/// Returns `(table, rows)`.
pub async fn restore_database(
    pool: &PgPool,
    migrator: &Migrator,
    bundle: &Bundle,
) -> Result<Vec<(String, u64)>> {
    check_migrations(&bundle.manifest.migrations, &known_migrations(migrator))?;
    migrator
        .run(pool)
        .await
        .context("failed to run database migrations")?;
    let mut tx = pool.begin().await?;
    let loaded = dump::import(&mut tx, &bundle.tables).await?;
    for t in &bundle.manifest.tables {
        let got = loaded
            .iter()
            .find(|(n, _)| *n == t.name)
            .map_or(0, |(_, r)| *r);
        ensure!(
            got == t.rows,
            "table {}: loaded {got} rows, the manifest says {}",
            t.name,
            t.rows
        );
    }
    tx.commit().await?;
    Ok(loaded)
}

/// The backup's configuration, split into the bootstrap keys for
/// `config.toml` (minus those the environment `env` sets, which keep coming
/// from there) and every other setting, for the database (all of them: the
/// environment overrides them at runtime anyway, and the restored server
/// keeps working if it's later started without those variables).
pub fn restored_config(
    bundle: &Bundle,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<(FileConfig, FileConfig)> {
    let file: FileConfig =
        toml::from_str(&bundle.config_toml).context("the backup's config.toml is invalid")?;
    let mut boot = file.bootstrap();
    for key in settings::apply_env(&FileConfig::default(), env)?.env_locked {
        settings::clear_key(&mut boot, key);
    }
    Ok((boot, file.without_bootstrap()))
}

fn print_manifest(source: &str, bundle: &Bundle) {
    let m = &bundle.manifest;
    eprintln!("Backup {source}");
    eprintln!("  made by:     OurShell server {}", m.server_version);
    eprintln!("  created:     {}", m.created_at.to_rfc3339());
    eprintln!("  migrations:  {:?}", m.migrations);
    eprintln!("  content:     sha256 {}", m.content_hash);
    for t in &m.tables {
        eprintln!("  {:<26} {:>8} rows", t.name, t.rows);
    }
}

async fn fetch(source: &str, current: &Effective) -> Result<(String, Vec<u8>)> {
    let Some(key) = source.strip_prefix("s3:") else {
        let bytes =
            std::fs::read(source).with_context(|| format!("can't read backup file {source}"))?;
        return Ok((source.to_string(), bytes));
    };
    let cfg = BackupConfig::from_file(current.values.backup.as_ref());
    let client = cfg.client().context(
        "s3: needs the backup storage settings (OURSHELL_BACKUP_S3_* variables, or this server's \
         settings from /admin -> Backups when its database is reachable)",
    )?;
    let key = if key == "latest" {
        let objects = client.list(&cfg.prefix).await?;
        let mut keys: Vec<String> = objects
            .into_iter()
            .map(|o| o.key)
            .filter(|k| {
                k.strip_prefix(cfg.prefix.as_str())
                    .is_some_and(format::is_backup_name)
            })
            .collect();
        keys.sort();
        keys.pop()
            .ok_or_else(|| anyhow!("no backups under {:?} in {}", cfg.prefix, client.bucket()))?
    } else if key.starts_with(cfg.prefix.as_str()) {
        key.to_string()
    } else {
        format!("{}{key}", cfg.prefix)
    };
    eprintln!("Downloading s3://{}/{key} ...", client.bucket());
    let bytes = client.get_object(&key).await?;
    Ok((format!("s3://{}/{key}", client.bucket()), bytes))
}

fn passphrase(current: &Effective) -> Result<String> {
    if let Some(p) = current
        .values
        .backup
        .as_ref()
        .and_then(|b| b.passphrase.clone())
        .filter(|p| !p.is_empty())
    {
        return Ok(p);
    }
    eprint!("Backup passphrase: ");
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let p = line.trim_end_matches(['\r', '\n']).to_string();
    ensure!(!p.is_empty(), "no passphrase given");
    Ok(p)
}

/// The CLI entry point; `args` are the words after `restore`.
pub async fn run_cli(args: &[String]) -> Result<()> {
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let rest: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if let Some(flag) = args
        .iter()
        .find(|a| a.starts_with("--") && *a != "--dry-run")
    {
        bail!("unknown option {flag}\n{USAGE}");
    }
    let [source] = rest.as_slice() else {
        bail!("{USAGE}");
    };
    let data_dir = settings::data_dir();
    // This server's current settings (environment, config.toml and, when
    // reachable, its database) for the S3 storage and the passphrase.
    let (current, store) = crate::startup::cli_settings(&data_dir).await?;
    if let Some(s) = store {
        s.pool.close().await;
    }
    let (label, bytes) = fetch(source, &current).await?;
    let pass = passphrase(&current)?;
    let bundle = tokio::task::spawn_blocking(move || format::open(&bytes, &pass))
        .await
        .map_err(|e| anyhow!("decryption task failed: {e}"))??;
    print_manifest(&label, &bundle);
    let migrator = crate::migrator().await?;
    check_migrations(&bundle.manifest.migrations, &known_migrations(&migrator))?;
    if dry_run {
        eprintln!("Dry run: the backup decrypts and is intact. Nothing was changed.");
        return Ok(());
    }

    let env = |k: &str| std::env::var(k).ok();
    let backup_file: FileConfig =
        toml::from_str(&bundle.config_toml).context("the backup's config.toml is invalid")?;
    let effective = settings::apply_env(&backup_file, &env)?;
    if let (Some(ours), Some(env_key)) = (
        backup_file.server_master_key.as_deref(),
        env("SERVER_MASTER_KEY").filter(|k| !k.trim().is_empty()),
    ) {
        ensure!(
            ours.trim() == env_key.trim(),
            "SERVER_MASTER_KEY in the environment differs from the backup's master key, so the \
             restored data couldn't be decrypted. Unset it (the backup's key is written to \
             config.toml) or set it to the backup's key, then run restore again."
        );
    }
    let database_url = effective
        .values
        .database_url
        .clone()
        .filter(|u| !u.is_empty())
        .ok_or_else(|| anyhow!("no database URL: set DATABASE_URL"))?;
    eprintln!(
        "Restoring into {} ...",
        crate::web::redact_database_url(&database_url)
    );
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .context("failed to connect to the database")?;
    let loaded = restore_database(&pool, &migrator, &bundle).await?;
    let rows: u64 = loaded.iter().map(|(_, r)| r).sum();
    eprintln!("Restored {} tables, {rows} rows.", loaded.len());

    // The backup's settings go into the restored database, encrypted with
    // the master key it was made with (the environment's, which must match).
    let (boot, restored_settings) = restored_config(&bundle, &env)?;
    let key_b64 = backup_file
        .server_master_key
        .clone()
        .or_else(|| env("SERVER_MASTER_KEY"))
        .filter(|k| !k.trim().is_empty())
        .ok_or_else(|| anyhow!("the backup has no master key; set SERVER_MASTER_KEY"))?;
    let key = crate::config::decode_master_key(&key_b64)?;
    settings::db::write(&pool, &key, &restored_settings).await?;
    eprintln!("Restored the server settings into the database.");

    let path = data_dir.join(settings::CONFIG_FILE);
    if path.exists() {
        let saved = settings::save_copy(&data_dir, "before-restore")
            .with_context(|| format!("failed to save {}", path.display()))?;
        eprintln!("Saved the previous config as {}", saved.display());
    }
    // config.toml keeps only the bootstrap keys; its old settings would
    // override the restored ones.
    let previous = settings::read(&data_dir).unwrap_or_default();
    let new_file = settings::merge(&previous.bootstrap(), &boot);
    match settings::write(&data_dir, &new_file) {
        Ok(()) => eprintln!("Wrote {}", path.display()),
        Err(e) => eprintln!("Couldn't write {}: {e:#}", path.display()),
    }
    // The old backup state describes the old database.
    let _ = std::fs::remove_file(data_dir.join(super::STATE_FILE));
    let persistence = crate::startup::data_dir_persistence(&data_dir);
    if !persistence.persistent && env("SERVER_MASTER_KEY").is_none() {
        eprintln!();
        eprintln!(
            "WARNING: {}. Add this to the server's environment, or the next redeploy",
            persistence.detail
        );
        eprintln!("loses the master key and the restored data can't be read:");
        eprintln!("  SERVER_MASTER_KEY={}", key_b64.trim());
    }
    eprintln!();
    eprintln!("Next: restart the server so it picks up the restored data and configuration,");
    eprintln!("e.g. `docker restart ourshell-server`. Accounts, devices and sign-ins are as they");
    eprintln!("were when the backup was made.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_backups_are_refused() {
        assert!(check_migrations(&[1, 2], &[1, 2, 3]).is_ok());
        assert!(check_migrations(&[1], &[1, 2]).is_ok());
        let err = check_migrations(&[1, 2, 3], &[1, 2]).unwrap_err();
        assert!(err.to_string().contains("[3]"), "{err}");
    }

    #[test]
    fn env_overridden_keys_stay_out_of_the_restored_config() {
        let mut b = format::tests::sample(chrono::Utc::now(), "s");
        b.config_toml = "database_url = \"postgres://backup\"\nserver_master_key = \"k\"\n\
                         public_url = \"https://old\"\n[backup]\nbucket = \"b\"\nkeep = 3\n"
            .into();
        let env = |k: &str| match k {
            "DATABASE_URL" => Some("postgres://env".to_string()),
            "OURSHELL_BACKUP_S3_BUCKET" => Some("env-bucket".to_string()),
            _ => None,
        };
        let (boot, f) = restored_config(&b, &env).unwrap();
        assert_eq!(boot.database_url, None);
        assert_eq!(boot.server_master_key.as_deref(), Some("k"));
        assert_eq!(boot.public_url, None, "settings go to the database");
        assert_eq!(
            f.server_master_key, None,
            "the key never goes to the database"
        );
        assert_eq!(f.public_url.as_deref(), Some("https://old"));
        let bk = f.backup.unwrap();
        assert_eq!((bk.bucket.as_deref(), bk.keep), (Some("b"), Some(3)));
    }
}
