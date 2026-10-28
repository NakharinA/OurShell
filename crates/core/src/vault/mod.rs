//! Encrypted local vault backed by SQLite.
//!
//! Only sync metadata (id, type, version, org id, timestamps) is stored in plain
//! text; every item body is an [`ItemEnvelope`] sealed with the vault key.

pub mod model;

use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::crypto::kdf::{derive_master_key, KdfParams};
use crate::crypto::keys::{
    b64, open_vault_key, seal_vault_key, unwrap_with_recovery, wrap_with_recovery, AccountKeys,
    AccountPublicKeys, ItemEnvelope,
};
use crate::crypto::{recovery, CryptoError, SymmetricKey};
pub use model::*;

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("vault is locked")]
    Locked,
    #[error("wrong master password")]
    WrongPassword,
    #[error("vault already exists at {0}")]
    Exists(PathBuf),
    #[error("no vault at {0}")]
    Missing(PathBuf),
    #[error("item not found: {0}")]
    NotFound(String),
    #[error("item {id} has type {actual}, expected {expected}")]
    WrongKind {
        id: String,
        actual: String,
        expected: &'static str,
    },
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, VaultError>;

/// Plain-text vault header: KDF parameters and wrapped keys. Contains no secrets.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Header {
    pub format: u8,
    pub org_id: String,
    pub vault_key_id: String,
    /// True for a vault that never syncs.
    pub local_only: bool,
    pub kdf: KdfParams,
    #[serde(with = "b64")]
    pub wrapped_account_keys: Vec<u8>,
    pub account_public: AccountPublicKeys,
    #[serde(with = "b64")]
    pub sealed_vault_key: Vec<u8>,
    #[serde(with = "b64")]
    pub recovery_wrap: Vec<u8>,
}

/// Sync metadata for one stored item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemMeta {
    pub id: String,
    pub kind: String,
    pub version: u64,
    pub updated_at: String,
    pub deleted: bool,
}

struct Unlocked {
    account: AccountKeys,
    vault_key: SymmetricKey,
}

pub struct Vault {
    path: PathBuf,
    conn: Connection,
    header: Header,
    unlocked: Option<Unlocked>,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS items (
    id         TEXT PRIMARY KEY,
    kind       TEXT NOT NULL,
    version    INTEGER NOT NULL,
    org_id     TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    deleted    INTEGER NOT NULL DEFAULT 0,
    dirty      INTEGER NOT NULL DEFAULT 1,
    envelope   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS items_kind ON items(kind);
"#;

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Created vault plus the recovery phrase, which must be shown to the user once.
pub struct Created {
    pub vault: Vault,
    pub recovery_phrase: Zeroizing<String>,
}

impl Vault {
    /// Default vault location: `<data dir>/ourshell/vault.db`.
    pub fn default_path() -> PathBuf {
        if let Ok(p) = std::env::var("OURSHELL_VAULT") {
            return PathBuf::from(p);
        }
        dirs_home().join(".ourshell").join("vault.db")
    }

    /// Creates a new vault, returning it unlocked.
    pub fn create(path: &Path, password: &str, local_only: bool) -> Result<Created> {
        Self::create_with_kdf(path, password, local_only, KdfParams::new_default()?)
    }

    pub fn create_with_kdf(
        path: &Path,
        password: &str,
        local_only: bool,
        kdf: KdfParams,
    ) -> Result<Created> {
        if path.exists() {
            return Err(VaultError::Exists(path.into()));
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| {
                VaultError::Db(rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
            })?;
        }
        let master = derive_master_key(password.as_bytes(), &kdf)?;
        let account = AccountKeys::generate()?;
        let vault_key = SymmetricKey::generate()?;
        let recovery_key = SymmetricKey::generate()?;
        let header = Header {
            format: 1,
            org_id: new_id(),
            vault_key_id: new_id(),
            local_only,
            kdf,
            wrapped_account_keys: account.wrap(&master)?,
            account_public: account.public(),
            sealed_vault_key: seal_vault_key(&account.public(), &vault_key)?,
            recovery_wrap: wrap_with_recovery(&recovery_key, &vault_key)?,
        };
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        let mut v = Self {
            path: path.into(),
            conn,
            header,
            unlocked: Some(Unlocked { account, vault_key }),
        };
        v.save_header()?;
        Ok(Created {
            vault: v,
            recovery_phrase: Zeroizing::new(recovery::to_words(&recovery_key)),
        })
    }

    /// Creates a new vault file from account/vault material already issued
    /// by a sync server (used when logging in on a new device: no local
    /// account keys or vault key need to be generated, they come from the
    /// server's wrapped/sealed copies). Returned locked; callers unlock
    /// with the master password to prove the material round-trips.
    pub fn create_from_header(path: &Path, header: Header) -> Result<Self> {
        if path.exists() {
            return Err(VaultError::Exists(path.into()));
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| {
                VaultError::Db(rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
            })?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        let mut v = Self {
            path: path.into(),
            conn,
            header,
            unlocked: None,
        };
        v.save_header()?;
        Ok(v)
    }

    /// Opens an existing vault in the locked state.
    pub fn open(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Err(VaultError::Missing(path.into()));
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        let raw: String = conn
            .query_row("SELECT v FROM meta WHERE k = 'header'", [], |r| r.get(0))
            .optional()?
            .ok_or_else(|| VaultError::Missing(path.into()))?;
        Ok(Self {
            path: path.into(),
            conn,
            header: serde_json::from_str(&raw)?,
            unlocked: None,
        })
    }

    fn save_header(&mut self) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta (k, v) VALUES ('header', ?1)
             ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            params![serde_json::to_string(&self.header)?],
        )?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn is_unlocked(&self) -> bool {
        self.unlocked.is_some()
    }

    pub fn unlock(&mut self, password: &str) -> Result<()> {
        let master = derive_master_key(password.as_bytes(), &self.header.kdf)?;
        let account = AccountKeys::unwrap(&master, &self.header.wrapped_account_keys)
            .map_err(|_| VaultError::WrongPassword)?;
        let vault_key = open_vault_key(&account, &self.header.sealed_vault_key)?;
        self.unlocked = Some(Unlocked { account, vault_key });
        Ok(())
    }

    /// Unlocks with the recovery phrase and sets a new master password.
    pub fn recover(&mut self, phrase: &str, new_password: &str) -> Result<()> {
        let rk = recovery::from_words(phrase)?;
        let vault_key = unwrap_with_recovery(&rk, &self.header.recovery_wrap)?;
        // The account keys are wrapped by the lost password, so issue new ones and
        // re-sign every item under them.
        let account = AccountKeys::generate()?;
        self.header.account_public = account.public();
        self.header.sealed_vault_key = seal_vault_key(&account.public(), &vault_key)?;
        self.unlocked = Some(Unlocked { account, vault_key });
        self.resign_all()?;
        self.change_password(new_password)
    }

    /// Re-wraps the account keys under a new master password (new salt).
    pub fn change_password(&mut self, new_password: &str) -> Result<()> {
        let u = self.unlocked.as_ref().ok_or(VaultError::Locked)?;
        let kdf = KdfParams {
            salt: {
                let mut s = [0u8; 16];
                crate::crypto::random_bytes(&mut s)?;
                s
            },
            ..self.header.kdf.clone()
        };
        let master = derive_master_key(new_password.as_bytes(), &kdf)?;
        self.header.wrapped_account_keys = u.account.wrap(&master)?;
        self.header.kdf = kdf;
        self.save_header()
    }

    pub fn lock(&mut self) {
        self.unlocked = None;
    }

    fn keys(&self) -> Result<&Unlocked> {
        self.unlocked.as_ref().ok_or(VaultError::Locked)
    }

    /// The account's signing/encryption keys (for sync and sharing).
    pub fn account_keys(&self) -> Result<&AccountKeys> {
        Ok(&self.keys()?.account)
    }

    pub fn vault_key(&self) -> Result<&SymmetricKey> {
        Ok(&self.keys()?.vault_key)
    }

    // ----- typed CRUD -----

    /// Inserts or updates an item. Returns its id.
    pub fn put<T: Item>(&mut self, id: Option<&str>, item: &T) -> Result<String> {
        let id = id.map(str::to_owned).unwrap_or_else(new_id);
        let plaintext = Zeroizing::new(serde_json::to_vec(item)?);
        self.put_raw(&id, T::KIND, &plaintext, false)?;
        Ok(id)
    }

    fn put_raw(&mut self, id: &str, kind: &str, plaintext: &[u8], deleted: bool) -> Result<()> {
        let current: Option<(String, u64)> = self
            .conn
            .query_row(
                "SELECT kind, version FROM items WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64)),
            )
            .optional()?;
        if let Some((k, _)) = &current {
            if k != kind {
                return Err(VaultError::NotFound(id.into()));
            }
        }
        let version = current.map(|(_, v)| v + 1).unwrap_or(1);
        let u = self.keys()?;
        let env = ItemEnvelope::seal(
            id,
            kind,
            version,
            &self.header.org_id,
            &self.header.vault_key_id,
            plaintext,
            &u.vault_key,
            &u.account.sign_secret,
        )?;
        self.store_envelope(&env, deleted, true)
    }

    fn store_envelope(&self, env: &ItemEnvelope, deleted: bool, dirty: bool) -> Result<()> {
        self.conn.execute(
            "INSERT INTO items (id, kind, version, org_id, updated_at, deleted, dirty, envelope)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET kind = excluded.kind, version = excluded.version,
               org_id = excluded.org_id, updated_at = excluded.updated_at,
               deleted = excluded.deleted, dirty = excluded.dirty, envelope = excluded.envelope",
            params![
                env.id,
                env.kind,
                env.version as i64,
                env.org_id,
                now(),
                deleted,
                dirty,
                serde_json::to_string(env)?
            ],
        )?;
        Ok(())
    }

    fn load_envelope(&self, id: &str) -> Result<Option<(ItemEnvelope, bool)>> {
        let row: Option<(String, bool)> = self
            .conn
            .query_row(
                "SELECT envelope, deleted FROM items WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match row {
            Some((s, d)) => Ok(Some((serde_json::from_str(&s)?, d))),
            None => Ok(None),
        }
    }

    fn decrypt_env<T: Item>(&self, env: &ItemEnvelope) -> Result<T> {
        let u = self.keys()?;
        let verifying = self.header.account_public.verifying()?;
        let pt = Zeroizing::new(env.open(&u.vault_key, &verifying)?);
        Ok(serde_json::from_slice(&pt)?)
    }

    pub fn get<T: Item>(&self, id: &str) -> Result<T> {
        match self.load_envelope(id)? {
            Some((env, false)) if env.kind == T::KIND => self.decrypt_env(&env),
            Some((env, false)) => Err(VaultError::WrongKind {
                id: id.into(),
                actual: env.kind,
                expected: T::KIND,
            }),
            _ => Err(VaultError::NotFound(id.into())),
        }
    }

    /// All live items of a type, as `(id, item)`.
    pub fn list<T: Item>(&self) -> Result<Vec<(String, T)>> {
        self.keys()?;
        let mut stmt = self
            .conn
            .prepare("SELECT envelope FROM items WHERE kind = ?1 AND deleted = 0 ORDER BY id")?;
        let rows = stmt
            .query_map(params![T::KIND], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.iter()
            .map(|s| {
                let env: ItemEnvelope = serde_json::from_str(s)?;
                Ok((env.id.clone(), self.decrypt_env(&env)?))
            })
            .collect()
    }

    /// Finds an item by id, or by its `name` field (case-insensitive) for named types.
    pub fn find<T: Item>(&self, id_or_name: &str) -> Result<(String, T)> {
        if let Ok(item) = self.get::<T>(id_or_name) {
            return Ok((id_or_name.into(), item));
        }
        for (id, item) in self.list::<T>()? {
            let v = serde_json::to_value(&item)?;
            if v.get("name")
                .and_then(|n| n.as_str())
                .is_some_and(|n| n.eq_ignore_ascii_case(id_or_name))
            {
                return Ok((id, item));
            }
        }
        Err(VaultError::NotFound(id_or_name.into()))
    }

    /// Soft-deletes an item (a tombstone is kept so the deletion can sync).
    pub fn delete(&mut self, id: &str) -> Result<()> {
        let (env, deleted) = self
            .load_envelope(id)?
            .ok_or_else(|| VaultError::NotFound(id.into()))?;
        if deleted {
            return Err(VaultError::NotFound(id.into()));
        }
        self.put_raw(id, &env.kind, b"null", true)
    }

    // ----- sync support -----

    /// Item metadata (including tombstones), for sync and diagnostics.
    pub fn metas(&self) -> Result<Vec<ItemMeta>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, kind, version, updated_at, deleted FROM items ORDER BY id")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(ItemMeta {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    version: r.get::<_, i64>(2)? as u64,
                    updated_at: r.get(3)?,
                    deleted: r.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Envelopes changed locally since the last sync, with their tombstone flag.
    pub fn dirty_envelopes(&self) -> Result<Vec<(ItemEnvelope, bool)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT envelope, deleted FROM items WHERE dirty = 1 ORDER BY id")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(s, d)| Ok((serde_json::from_str(&s)?, d)))
            .collect()
    }

    pub fn mark_clean(&self, id: &str, version: u64) -> Result<()> {
        self.conn.execute(
            "UPDATE items SET dirty = 0 WHERE id = ?1 AND version = ?2",
            params![id, version as i64],
        )?;
        Ok(())
    }

    /// Stores an envelope received from the server after verifying it decrypts.
    /// Returns false if the local copy is newer or equal.
    pub fn apply_remote(&mut self, env: &ItemEnvelope, deleted: bool) -> Result<bool> {
        let u = self.keys()?;
        let verifying = self.header.account_public.verifying()?;
        env.open(&u.vault_key, &verifying)?;
        if let Some((local, _)) = self.load_envelope(&env.id)? {
            if local.version >= env.version {
                return Ok(false);
            }
        }
        self.store_envelope(env, deleted, false)?;
        Ok(true)
    }

    /// Forcibly stores a remote envelope as this item's current version,
    /// bypassing the "only if strictly newer" check in [`Self::apply_remote`].
    /// Used after a push conflict: the server's copy is authoritative even
    /// though the local and remote version numbers can coincide (both were
    /// bumped independently from the same base version by different
    /// devices). Still verifies the signature before storing.
    pub fn force_apply_remote(&mut self, env: &ItemEnvelope, deleted: bool) -> Result<()> {
        let u = self.keys()?;
        let verifying = self.header.account_public.verifying()?;
        env.open(&u.vault_key, &verifying)?;
        self.store_envelope(env, deleted, false)
    }

    /// Stores a raw plaintext under an explicit id/kind as a new local version
    /// (used for conflict copies during sync).
    pub fn put_plain(&mut self, id: &str, kind: &str, plaintext: &[u8]) -> Result<()> {
        self.put_raw(id, kind, plaintext, false)
    }

    const SECRET_META_PREFIX: &'static str = "secret:";
    const SECRET_META_AAD_PREFIX: &'static str = "ourshell:meta:";

    /// Stores `bytes` encrypted under the vault key, keyed by `key`, in this
    /// vault's own `meta` table. For client-only state that must never
    /// leave the device (e.g. sync session tokens) -- never synced, unlike
    /// `items`.
    pub fn set_secret_meta(&mut self, key: &str, bytes: &[u8]) -> Result<()> {
        let u = self.keys()?;
        let aad = format!("{}{key}", Self::SECRET_META_AAD_PREFIX);
        let enc = crate::crypto::aead::encrypt(&u.vault_key, bytes, aad.as_bytes())?;
        self.conn.execute(
            "INSERT INTO meta (k, v) VALUES (?1, ?2)
             ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            params![
                format!("{}{key}", Self::SECRET_META_PREFIX),
                STANDARD.encode(enc)
            ],
        )?;
        Ok(())
    }

    /// Reads back a value stored with [`Self::set_secret_meta`], or `None`
    /// if never set.
    pub fn get_secret_meta(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let u = self.keys()?;
        let row: Option<String> = self
            .conn
            .query_row(
                "SELECT v FROM meta WHERE k = ?1",
                params![format!("{}{key}", Self::SECRET_META_PREFIX)],
                |r| r.get(0),
            )
            .optional()?;
        let Some(encoded) = row else {
            return Ok(None);
        };
        let enc = STANDARD
            .decode(&encoded)
            .map_err(|_| VaultError::Crypto(CryptoError::Format))?;
        let aad = format!("{}{key}", Self::SECRET_META_AAD_PREFIX);
        Ok(Some(crate::crypto::aead::decrypt(
            &u.vault_key,
            &enc,
            aad.as_bytes(),
        )?))
    }

    /// Removes a value stored with [`Self::set_secret_meta`].
    pub fn delete_secret_meta(&mut self, key: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM meta WHERE k = ?1",
            params![format!("{}{key}", Self::SECRET_META_PREFIX)],
        )?;
        Ok(())
    }

    /// Decrypts any envelope with this vault's keys.
    pub fn open_envelope(&self, env: &ItemEnvelope) -> Result<Zeroizing<Vec<u8>>> {
        let u = self.keys()?;
        let verifying = self.header.account_public.verifying()?;
        Ok(Zeroizing::new(env.open(&u.vault_key, &verifying)?))
    }

    /// Replaces the header (used when a synced account replaces local keys).
    pub fn replace_header(&mut self, header: Header) -> Result<()> {
        self.header = header;
        self.unlocked = None;
        self.save_header()
    }

    pub fn set_local_only(&mut self, local_only: bool) -> Result<()> {
        self.header.local_only = local_only;
        self.save_header()
    }

    fn resign_all(&mut self) -> Result<()> {
        let mut stmt = self.conn.prepare("SELECT envelope, deleted FROM items")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(stmt);
        for (s, deleted) in rows {
            let env: ItemEnvelope = serde_json::from_str(&s)?;
            let u = self.keys()?;
            let item_key = env.unwrap_item_key(&u.vault_key)?;
            let pt = Zeroizing::new(crate::crypto::aead::decrypt(
                &item_key,
                &env.ciphertext,
                format!("{}|{}|{}", env.id, env.kind, env.version).as_bytes(),
            )?);
            self.put_raw(&env.id, &env.kind, &pt, deleted)?;
        }
        Ok(())
    }
}

fn dirs_home() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_vault(dir: &tempfile::TempDir) -> Created {
        Vault::create_with_kdf(
            &dir.path().join("v.db"),
            "correct horse",
            true,
            KdfParams::insecure_for_tests(),
        )
        .unwrap()
    }

    fn host(name: &str) -> Host {
        Host {
            name: name.into(),
            hostname: "10.0.0.5".into(),
            port: 22,
            username: Some("root".into()),
            ..Default::default()
        }
    }

    #[test]
    fn create_lock_unlock_crud() {
        let dir = tempfile::tempdir().unwrap();
        let Created { mut vault, .. } = new_vault(&dir);
        let id = vault.put(None, &host("web")).unwrap();
        assert_eq!(vault.get::<Host>(&id).unwrap().name, "web");

        vault.lock();
        assert!(matches!(vault.get::<Host>(&id), Err(VaultError::Locked)));
        assert!(matches!(
            vault.unlock("wrong"),
            Err(VaultError::WrongPassword)
        ));
        vault.unlock("correct horse").unwrap();

        let mut h = vault.get::<Host>(&id).unwrap();
        h.port = 2222;
        vault.put(Some(&id), &h).unwrap();
        assert_eq!(vault.get::<Host>(&id).unwrap().port, 2222);
        assert_eq!(vault.metas().unwrap()[0].version, 2);

        assert_eq!(vault.find::<Host>("WEB").unwrap().0, id);
        assert!(matches!(
            vault.get::<Snippet>(&id),
            Err(VaultError::WrongKind { .. })
        ));
        vault.delete(&id).unwrap();
        assert!(vault.list::<Host>().unwrap().is_empty());
        assert!(vault.metas().unwrap()[0].deleted);
    }

    #[test]
    fn reopen_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let id = {
            let Created { mut vault, .. } = new_vault(&dir);
            vault.put(None, &host("db")).unwrap()
        };
        let mut v = Vault::open(&dir.path().join("v.db")).unwrap();
        assert!(!v.is_unlocked());
        v.unlock("correct horse").unwrap();
        assert_eq!(v.get::<Host>(&id).unwrap().name, "db");
    }

    #[test]
    fn plaintext_not_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let Created { mut vault, .. } = new_vault(&dir);
        vault
            .put(
                None,
                &Identity {
                    name: "id".into(),
                    password: Some("SuperSecretPw123".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        drop(vault);
        let bytes = std::fs::read(dir.path().join("v.db")).unwrap();
        let hay = String::from_utf8_lossy(&bytes);
        assert!(!hay.contains("SuperSecretPw123"));
        assert!(!hay.contains("correct horse"));
    }

    #[test]
    fn recovery_resets_password() {
        let dir = tempfile::tempdir().unwrap();
        let Created {
            mut vault,
            recovery_phrase,
        } = new_vault(&dir);
        let id = vault.put(None, &host("x")).unwrap();
        vault.lock();
        vault.recover(&recovery_phrase, "new pw").unwrap();
        vault.lock();
        assert!(vault.unlock("correct horse").is_err());
        vault.unlock("new pw").unwrap();
        assert_eq!(vault.get::<Host>(&id).unwrap().name, "x");
    }

    #[test]
    fn tampered_row_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let Created { mut vault, .. } = new_vault(&dir);
        let id = vault.put(None, &host("x")).unwrap();
        vault
            .conn
            .execute(
                "UPDATE items SET envelope = replace(envelope, '\"version\":1', '\"version\":7')",
                [],
            )
            .unwrap();
        assert!(matches!(
            vault.get::<Host>(&id),
            Err(VaultError::Crypto(CryptoError::BadSignature))
        ));
    }
}
