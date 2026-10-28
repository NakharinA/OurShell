//! Encrypted local vault backed by SQLite.
//!
//! Only sync metadata (id, type, version, org id, timestamps) is stored in plain
//! text; every item body is an [`ItemEnvelope`] sealed with the vault key.

pub mod model;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use ed25519_dalek::VerifyingKey;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::crypto::kdf::{derive_master_key, KdfParams};
use crate::crypto::keys::{
    b64, open_vault_key, opt_b64, seal_vault_key, unwrap_account_with_recovery,
    unwrap_with_recovery, wrap_account_with_recovery, wrap_with_recovery, AccountKeys,
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
    /// No sealed key for this vault id -- either this device was never a
    /// member, or it was removed (key rotation forgets the old key: see
    /// [`Vault::forget_vault`]).
    #[error("no key for vault `{0}` (not a member, or removed)")]
    UnknownVault(String),
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
    /// The account's enc/sign secret keys, wrapped a second time under the
    /// recovery key. Present on every vault created from this point on;
    /// `None` on a vault created before this field existed (such a vault's
    /// `recover()` falls back to the old regenerate-and-resign behaviour --
    /// see `docs/self-hosting.md`).
    #[serde(default, with = "opt_b64")]
    pub recovery_account_wrap: Option<Vec<u8>>,
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
    /// Every vault key this account currently has a sealed copy for,
    /// opened at unlock time: `vault_id -> (key_id, vault key)`. Always
    /// has at least the personal vault's entry (`header.org_id`).
    vault_keys: HashMap<String, (String, SymmetricKey)>,
}

pub struct Vault {
    path: PathBuf,
    conn: Connection,
    header: Header,
    unlocked: Option<Unlocked>,
}

/// One vault (personal or team) this account has a key for, as recorded
/// locally in the `vaults` table. See [`Vault::vaults`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultInfo {
    pub vault_id: String,
    pub key_id: String,
    pub name: String,
    /// "personal" or "team".
    pub kind: String,
    pub role: String,
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
-- Every (personal or team) vault this account currently holds a sealed key
-- for. `sealed_vault_key` is that vault's key sealed to this account's own
-- X25519 public key (same sealing `header.sealed_vault_key` uses for the
-- personal vault) -- opening it needs nothing but the account keys, so
-- `Vault::unlock` opens every row here in one pass. See docs/blob-format.md.
CREATE TABLE IF NOT EXISTS vaults (
    vault_id         TEXT PRIMARY KEY,
    key_id           TEXT NOT NULL,
    name             TEXT NOT NULL,
    kind             TEXT NOT NULL,
    sealed_vault_key BLOB NOT NULL,
    role             TEXT NOT NULL DEFAULT 'member'
);
-- Ed25519 sign-public keys of every current member of a (team) vault,
-- fetched from the server. An item's signature is accepted if it
-- verifies against *any* row for its vault (membership changes over
-- time; see docs/blob-format.md for why this is preferred over an
-- explicit `signer` field on the envelope).
CREATE TABLE IF NOT EXISTS vault_signers (
    vault_id    TEXT NOT NULL,
    sign_public TEXT NOT NULL,
    PRIMARY KEY (vault_id, sign_public)
);
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
        let vault_key_id = new_id();
        let sealed_vault_key = seal_vault_key(&account.public(), &vault_key)?;
        let header = Header {
            format: 1,
            org_id: new_id(),
            vault_key_id,
            local_only,
            kdf,
            wrapped_account_keys: account.wrap(&master)?,
            account_public: account.public(),
            sealed_vault_key,
            recovery_wrap: wrap_with_recovery(&recovery_key, &vault_key)?,
            recovery_account_wrap: Some(wrap_account_with_recovery(&recovery_key, &account)?),
        };
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        let mut v = Self {
            path: path.into(),
            conn,
            header,
            unlocked: None,
        };
        v.save_header()?;
        v.migrate_vaults_table()?;
        let vault_keys = v.open_all_vault_keys(&account)?;
        v.unlocked = Some(Unlocked {
            account,
            vault_keys,
        });
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
        v.migrate_vaults_table()?;
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
        let mut v = Self {
            path: path.into(),
            conn,
            header: serde_json::from_str(&raw)?,
            unlocked: None,
        };
        v.migrate_vaults_table()?;
        Ok(v)
    }

    /// One-time, idempotent migration run on every open/create: makes sure
    /// `items` has a `vault_id` column (backfilled to the personal vault
    /// for rows that predate multi-vault support) and that the personal
    /// vault has a row in `vaults`.
    fn migrate_vaults_table(&mut self) -> Result<()> {
        let has_vault_id_col = self
            .conn
            .prepare("SELECT vault_id FROM items LIMIT 0")
            .is_ok();
        if !has_vault_id_col {
            self.conn
                .execute_batch("ALTER TABLE items ADD COLUMN vault_id TEXT NOT NULL DEFAULT ''")?;
            self.conn.execute(
                "CREATE INDEX IF NOT EXISTS items_vault ON items(vault_id)",
                [],
            )?;
        }
        self.conn.execute(
            "UPDATE items SET vault_id = ?1 WHERE vault_id = ''",
            params![self.header.org_id],
        )?;
        self.conn.execute(
            "INSERT INTO vaults (vault_id, key_id, name, kind, sealed_vault_key, role)
             VALUES (?1, ?2, 'Personal', 'personal', ?3, 'owner')
             ON CONFLICT(vault_id) DO NOTHING",
            params![
                self.header.org_id,
                self.header.vault_key_id,
                self.header.sealed_vault_key
            ],
        )?;
        Ok(())
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
        let vault_keys = self.open_all_vault_keys(&account)?;
        self.unlocked = Some(Unlocked {
            account,
            vault_keys,
        });
        Ok(())
    }

    /// Opens every sealed vault key in the `vaults` table with `account`'s
    /// X25519 secret key.
    fn open_all_vault_keys(
        &self,
        account: &AccountKeys,
    ) -> Result<HashMap<String, (String, SymmetricKey)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT vault_id, key_id, sealed_vault_key FROM vaults")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut map = HashMap::with_capacity(rows.len());
        for (vault_id, key_id, sealed) in rows {
            let vk = open_vault_key(account, &sealed)?;
            map.insert(vault_id, (key_id, vk));
        }
        Ok(map)
    }

    /// Unlocks with the recovery phrase and sets a new master password.
    ///
    /// If this vault has `header.recovery_account_wrap` (every vault
    /// created from this point on does), the account's enc/sign keys are
    /// restored *unchanged* -- no item needs re-signing, and a sync server
    /// or other device never sees a different verifying key. Only the
    /// password-derived wrapping changes.
    ///
    /// Vaults created before that field existed fall back to the original
    /// behaviour: a brand-new account keypair is generated and every item
    /// is re-signed under it. This desyncs a vault that is signed in to a
    /// sync server (see docs/self-hosting.md) -- callers should treat that
    /// case specially (re-register the account keys with the server).
    pub fn recover(&mut self, phrase: &str, new_password: &str) -> Result<()> {
        let rk = recovery::from_words(phrase)?;
        let vault_key = unwrap_with_recovery(&rk, &self.header.recovery_wrap)?;

        if let Some(wrapped) = self.header.recovery_account_wrap.clone() {
            let account = unwrap_account_with_recovery(&rk, &wrapped)?;
            let vault_keys = self.open_all_vault_keys(&account)?;
            self.unlocked = Some(Unlocked {
                account,
                vault_keys,
            });
            return self.change_password(new_password);
        }

        // Legacy vault: the account keys are wrapped by the lost password
        // with no recovery copy, so issue new ones and re-sign every item
        // under them.
        let account = AccountKeys::generate()?;
        self.header.account_public = account.public();
        self.header.sealed_vault_key = seal_vault_key(&account.public(), &vault_key)?;
        self.conn.execute(
            "UPDATE vaults SET sealed_vault_key = ?1 WHERE vault_id = ?2",
            params![self.header.sealed_vault_key, self.header.org_id],
        )?;
        let vault_keys = self.open_all_vault_keys(&account)?;
        self.unlocked = Some(Unlocked {
            account,
            vault_keys,
        });
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

    /// This vault's current KDF params, wrapped account keys, and (if
    /// present) the account keys' recovery wrap -- what [`crate::sync`]
    /// needs to push to a server after [`Self::change_password`] or
    /// [`Self::recover`] so it (and every other device) stays in sync.
    pub fn account_key_material(&self) -> (KdfParams, Vec<u8>, Option<Vec<u8>>) {
        (
            self.header.kdf.clone(),
            self.header.wrapped_account_keys.clone(),
            self.header.recovery_account_wrap.clone(),
        )
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

    /// The personal vault's key (the one `header.org_id` names).
    pub fn vault_key(&self) -> Result<&SymmetricKey> {
        self.vault_key_for(&self.header.org_id)
    }

    /// The key for a specific (personal or team) vault this account has a
    /// sealed copy for.
    pub fn vault_key_for(&self, vault_id: &str) -> Result<&SymmetricKey> {
        self.keys()?
            .vault_keys
            .get(vault_id)
            .map(|(_, k)| k)
            .ok_or_else(|| VaultError::UnknownVault(vault_id.to_string()))
    }

    fn personal_vault_key(&self) -> Result<&SymmetricKey> {
        self.vault_key_for(&self.header.org_id)
    }

    /// Every (personal or team) vault this account currently has a key
    /// for.
    pub fn vaults(&self) -> Result<Vec<VaultInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT vault_id, key_id, name, kind, role FROM vaults ORDER BY kind DESC, name",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(VaultInfo {
                    vault_id: r.get(0)?,
                    key_id: r.get(1)?,
                    name: r.get(2)?,
                    kind: r.get(3)?,
                    role: r.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Records (or updates) a sealed vault key this account can open --
    /// after creating/joining a team vault, after a key rotation, or
    /// whenever sync learns the server's current vault list. If the vault
    /// is unlocked, the key is opened immediately so it's usable right
    /// away.
    pub fn upsert_vault(
        &mut self,
        vault_id: &str,
        key_id: &str,
        name: &str,
        kind: &str,
        sealed_vault_key: &[u8],
        role: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO vaults (vault_id, key_id, name, kind, sealed_vault_key, role)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(vault_id) DO UPDATE SET key_id = excluded.key_id, name = excluded.name,
               kind = excluded.kind, sealed_vault_key = excluded.sealed_vault_key, role = excluded.role",
            params![vault_id, key_id, name, kind, sealed_vault_key, role],
        )?;
        if let Some(u) = self.unlocked.as_mut() {
            let vk = open_vault_key(&u.account, sealed_vault_key)?;
            u.vault_keys
                .insert(vault_id.to_string(), (key_id.to_string(), vk));
        }
        Ok(())
    }

    /// Forgets a vault key (e.g. this device/account was removed from a
    /// team vault, or a rotation superseded it). Items already stored
    /// under it become locally undecryptable -- that's the point of
    /// rotation.
    pub fn forget_vault(&mut self, vault_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM vaults WHERE vault_id = ?1", params![vault_id])?;
        self.conn.execute(
            "DELETE FROM vault_signers WHERE vault_id = ?1",
            params![vault_id],
        )?;
        // Nothing can decrypt these any more (that's the point), and
        // leaving them around would make `list_all`/`get` fail on them
        // instead of simply no longer seeing them.
        self.conn
            .execute("DELETE FROM items WHERE vault_id = ?1", params![vault_id])?;
        if let Some(u) = self.unlocked.as_mut() {
            u.vault_keys.remove(vault_id);
        }
        Ok(())
    }

    /// Replaces the known member-signer set for a (team) vault, as
    /// base64-encoded Ed25519 public keys fetched from the server. See
    /// `docs/blob-format.md`.
    pub fn set_vault_signers(&mut self, vault_id: &str, sign_publics_b64: &[String]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM vault_signers WHERE vault_id = ?1",
            params![vault_id],
        )?;
        for s in sign_publics_b64 {
            tx.execute(
                "INSERT OR IGNORE INTO vault_signers (vault_id, sign_public) VALUES (?1, ?2)",
                params![vault_id, s],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The set of Ed25519 verifying keys accepted for items in `vault_id`:
    /// this account's own key, plus every current member's key on record
    /// for a team vault (see [`Self::set_vault_signers`]).
    fn acceptable_signers(&self, vault_id: &str) -> Result<Vec<VerifyingKey>> {
        let mut out = vec![self.header.account_public.verifying()?];
        let mut stmt = self
            .conn
            .prepare("SELECT sign_public FROM vault_signers WHERE vault_id = ?1")?;
        let rows = stmt
            .query_map(params![vault_id], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for s in rows {
            let bytes = STANDARD
                .decode(&s)
                .map_err(|_| VaultError::Crypto(CryptoError::Format))?;
            let arr: [u8; 32] = bytes
                .try_into()
                .map_err(|_| VaultError::Crypto(CryptoError::Format))?;
            out.push(
                VerifyingKey::from_bytes(&arr)
                    .map_err(|_| VaultError::Crypto(CryptoError::Format))?,
            );
        }
        Ok(out)
    }

    // ----- typed CRUD -----

    /// Inserts or updates an item in the personal vault. Returns its id.
    pub fn put<T: Item>(&mut self, id: Option<&str>, item: &T) -> Result<String> {
        let vault_id = self.header.org_id.clone();
        self.put_in(&vault_id, id, item)
    }

    /// Like [`Self::put`], but into a specific (personal or team) vault
    /// this account has a key for.
    pub fn put_in<T: Item>(
        &mut self,
        vault_id: &str,
        id: Option<&str>,
        item: &T,
    ) -> Result<String> {
        let id = id.map(str::to_owned).unwrap_or_else(new_id);
        let plaintext = Zeroizing::new(serde_json::to_vec(item)?);
        self.put_raw(&id, T::KIND, &plaintext, false, vault_id)?;
        Ok(id)
    }

    fn put_raw(
        &mut self,
        id: &str,
        kind: &str,
        plaintext: &[u8],
        deleted: bool,
        vault_id: &str,
    ) -> Result<()> {
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
        let (key_id, vkey) = u
            .vault_keys
            .get(vault_id)
            .ok_or_else(|| VaultError::UnknownVault(vault_id.to_string()))?;
        let env = ItemEnvelope::seal(
            id,
            kind,
            version,
            vault_id,
            key_id,
            plaintext,
            vkey,
            &u.account.sign_secret,
        )?;
        self.store_envelope(&env, deleted, true)
    }

    fn store_envelope(&self, env: &ItemEnvelope, deleted: bool, dirty: bool) -> Result<()> {
        self.conn.execute(
            "INSERT INTO items (id, kind, version, org_id, vault_id, updated_at, deleted, dirty, envelope)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET kind = excluded.kind, version = excluded.version,
               org_id = excluded.org_id, vault_id = excluded.vault_id, updated_at = excluded.updated_at,
               deleted = excluded.deleted, dirty = excluded.dirty, envelope = excluded.envelope",
            params![
                env.id,
                env.kind,
                env.version as i64,
                env.org_id,
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
        let pt = self.open_envelope(env)?;
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

    /// Which vault (personal or team) item `id` currently lives in.
    pub fn vault_of(&self, id: &str) -> Result<String> {
        match self.load_envelope(id)? {
            Some((env, false)) => Ok(env.org_id),
            _ => Err(VaultError::NotFound(id.into())),
        }
    }

    /// All live items of a type across every vault this account can open,
    /// as `(id, vault_id, item)`.
    pub fn list_all<T: Item>(&self) -> Result<Vec<(String, String, T)>> {
        self.keys()?;
        let mut stmt = self
            .conn
            .prepare("SELECT envelope FROM items WHERE kind = ?1 AND deleted = 0 ORDER BY id")?;
        let rows = stmt
            .query_map(params![T::KIND], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut out = Vec::with_capacity(rows.len());
        for s in &rows {
            let env: ItemEnvelope = serde_json::from_str(s)?;
            let vault_id = env.org_id.clone();
            // A stray item whose vault key this account no longer has
            // (e.g. a leftover from a vault it was just removed from,
            // before `forget_vault` swept it) is skipped rather than
            // failing the whole listing.
            match self.decrypt_env(&env) {
                Ok(item) => out.push((env.id.clone(), vault_id, item)),
                Err(VaultError::UnknownVault(_)) | Err(VaultError::Crypto(_)) => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(out)
    }

    /// All live items of a type in the personal vault, as `(id, item)`.
    /// Use [`Self::list_all`] to include team vaults too.
    pub fn list<T: Item>(&self) -> Result<Vec<(String, T)>> {
        let personal = self.header.org_id.clone();
        Ok(self
            .list_all::<T>()?
            .into_iter()
            .filter(|(_, vault_id, _)| vault_id == &personal)
            .map(|(id, _, item)| (id, item))
            .collect())
    }

    /// Finds an item by id, or by its `name` field (case-insensitive) for
    /// named types, searching every vault this account can open.
    pub fn find<T: Item>(&self, id_or_name: &str) -> Result<(String, T)> {
        if let Ok(item) = self.get::<T>(id_or_name) {
            return Ok((id_or_name.into(), item));
        }
        for (id, _vault_id, item) in self.list_all::<T>()? {
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
        let vault_id = env.org_id.clone();
        self.put_raw(id, &env.kind, b"null", true, &vault_id)
    }

    /// Moves item `id` into `team_vault_id` by re-wrapping its existing
    /// item key (the body is untouched: see
    /// [`crate::crypto::keys::ItemEnvelope::rewrap`]) and re-signing under
    /// this account. On success, returns a signed tombstone for the *old*
    /// vault (since locally there is only one row per item id, this vault
    /// no longer tracks a pending delete there on its own once the row has
    /// moved) -- the caller is responsible for pushing it (see
    /// [`crate::sync::share_item`]). Returns `None` if `id` is already in
    /// `team_vault_id`.
    /// Copies item `id` into `team_vault_id` under a *fresh* id and
    /// tombstones the original in place. Both writes go through the
    /// normal dirty-queue mechanism, so a single subsequent `sync` pushes
    /// both the new copy and the old vault's tombstone.
    ///
    /// A fresh id (rather than moving the same id across vaults) is
    /// required, not just simpler: both the item key's wrap and the body
    /// ciphertext bind `id` into their AAD (see
    /// [`crate::crypto::keys::ItemEnvelope`]), and this vault's local
    /// storage is keyed by a single global `id`, so one id can't
    /// correctly represent "deleted in vault A, live in vault B" at once.
    /// Returns the new id, or `id` unchanged if it's already in
    /// `team_vault_id`.
    pub fn share_into_vault(&mut self, id: &str, team_vault_id: &str) -> Result<String> {
        let (env, deleted) = self
            .load_envelope(id)?
            .ok_or_else(|| VaultError::NotFound(id.into()))?;
        if deleted {
            return Err(VaultError::NotFound(id.into()));
        }
        if env.org_id == team_vault_id {
            return Ok(id.to_string());
        }
        let plaintext = self.open_envelope(&env)?;
        let new_item_id = new_id();
        self.put_raw(&new_item_id, &env.kind, &plaintext, false, team_vault_id)?;
        let old_vault_id = env.org_id.clone();
        self.put_raw(id, &env.kind, b"null", true, &old_vault_id)?;
        Ok(new_item_id)
    }

    /// Rotates the key for a (team) vault: every item (including
    /// tombstones) is decrypted under the *current* key and re-saved as a
    /// new version under a fresh one. Old copies -- and anyone who was not
    /// re-sealed a copy of the new key, such as a just-removed member --
    /// stop being able to decrypt anything from this point on. This is how
    /// removing a team member takes effect; see `docs/self-hosting.md`.
    ///
    /// `new_sealed_vault_key` must be this account's own sealed copy of
    /// the freshly generated key (the caller seals one per remaining
    /// member and uploads those separately).
    pub fn rotate_vault_key(
        &mut self,
        vault_id: &str,
        new_key_id: &str,
        new_sealed_vault_key: &[u8],
    ) -> Result<Vec<String>> {
        let old_key = self.vault_key_for(vault_id)?.clone();
        let verifiers = self.acceptable_signers(vault_id)?;

        let mut stmt = self
            .conn
            .prepare("SELECT id, kind, envelope, deleted FROM items WHERE vault_id = ?1")?;
        let rows: Vec<(String, String, String, bool)> = stmt
            .query_map(params![vault_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(stmt);

        let mut plaintexts = Vec::with_capacity(rows.len());
        for (id, kind, s, deleted) in rows {
            let env: ItemEnvelope = serde_json::from_str(&s)?;
            let pt = if deleted {
                None
            } else {
                Some(Zeroizing::new(env.open_multi(&old_key, &verifiers)?))
            };
            plaintexts.push((id, kind, pt, deleted));
        }

        // Swap in the new key -- from here on, `old_key` is the only way
        // to read anything still sealed under the old one.
        let (name, kind_col, role): (String, String, String) = self.conn.query_row(
            "SELECT name, kind, role FROM vaults WHERE vault_id = ?1",
            params![vault_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        self.upsert_vault(
            vault_id,
            new_key_id,
            &name,
            &kind_col,
            new_sealed_vault_key,
            &role,
        )?;

        let mut touched = Vec::with_capacity(plaintexts.len());
        for (id, kind, pt, deleted) in plaintexts {
            match pt {
                Some(pt) => self.put_raw(&id, &kind, &pt, false, vault_id)?,
                None => self.put_raw(&id, &kind, b"null", deleted, vault_id)?,
            }
            touched.push(id);
        }
        Ok(touched)
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

    /// Verifies `env`'s signature (against any current member of its
    /// vault) and that its vault key is known, without decrypting.
    fn verify_envelope(&self, env: &ItemEnvelope) -> Result<()> {
        let vkey = self.vault_key_for(&env.org_id)?;
        let verifiers = self.acceptable_signers(&env.org_id)?;
        env.open_multi(vkey, &verifiers)?;
        Ok(())
    }

    /// Stores an envelope received from the server after verifying it decrypts.
    /// Returns false if the local copy is newer or equal.
    pub fn apply_remote(&mut self, env: &ItemEnvelope, deleted: bool) -> Result<bool> {
        self.verify_envelope(env)?;
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
        self.verify_envelope(env)?;
        self.store_envelope(env, deleted, false)
    }

    /// Stores a raw plaintext under an explicit id/kind as a new local
    /// version in `vault_id` (used for conflict copies during sync).
    pub fn put_plain(
        &mut self,
        id: &str,
        kind: &str,
        vault_id: &str,
        plaintext: &[u8],
    ) -> Result<()> {
        self.put_raw(id, kind, plaintext, false, vault_id)
    }

    const SECRET_META_PREFIX: &'static str = "secret:";
    const SECRET_META_AAD_PREFIX: &'static str = "ourshell:meta:";

    /// Stores `bytes` encrypted under the vault key, keyed by `key`, in this
    /// vault's own `meta` table. For client-only state that must never
    /// leave the device (e.g. sync session tokens) -- never synced, unlike
    /// `items`.
    pub fn set_secret_meta(&mut self, key: &str, bytes: &[u8]) -> Result<()> {
        let vkey = self.personal_vault_key()?;
        let aad = format!("{}{key}", Self::SECRET_META_AAD_PREFIX);
        let enc = crate::crypto::aead::encrypt(vkey, bytes, aad.as_bytes())?;
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
        let vkey = self.personal_vault_key()?;
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
            vkey,
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

    /// Decrypts any envelope with this vault's keys, verifying it against
    /// any current member of its vault.
    pub fn open_envelope(&self, env: &ItemEnvelope) -> Result<Zeroizing<Vec<u8>>> {
        let vkey = self.vault_key_for(&env.org_id)?;
        let verifiers = self.acceptable_signers(&env.org_id)?;
        Ok(Zeroizing::new(env.open_multi(vkey, &verifiers)?))
    }

    /// Replaces the header (used when a synced account replaces local keys).
    pub fn replace_header(&mut self, header: Header) -> Result<()> {
        self.header = header;
        self.unlocked = None;
        self.save_header()?;
        self.migrate_vaults_table()
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
            let vault_id = env.org_id.clone();
            let u = self.keys()?;
            let vkey = u
                .vault_keys
                .get(&vault_id)
                .ok_or_else(|| VaultError::UnknownVault(vault_id.clone()))?
                .1
                .clone();
            let item_key = env.unwrap_item_key(&vkey)?;
            let pt = Zeroizing::new(crate::crypto::aead::decrypt(
                &item_key,
                &env.ciphertext,
                format!("{}|{}|{}", env.id, env.kind, env.version).as_bytes(),
            )?);
            self.put_raw(&env.id, &env.kind, &pt, deleted, &vault_id)?;
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

    #[test]
    fn unlock_opens_every_vault_key() {
        let dir = tempfile::tempdir().unwrap();
        let Created { mut vault, .. } = new_vault(&dir);
        let personal_id = vault.header().org_id.clone();
        assert!(vault.vault_key_for(&personal_id).is_ok());

        // Simulate joining a team vault: a key for it, sealed to this
        // account's own public key (as a real share would be).
        let team_id = "team-1".to_string();
        let team_key = SymmetricKey::generate().unwrap();
        let sealed = seal_vault_key(&vault.account_keys().unwrap().public(), &team_key).unwrap();
        vault
            .upsert_vault(&team_id, "team-key-1", "Ops", "team", &sealed, "member")
            .unwrap();
        assert!(vault.vault_key_for(&team_id).is_ok());
        assert_eq!(vault.vaults().unwrap().len(), 2);

        // Locking and unlocking again re-derives both keys from scratch.
        vault.lock();
        assert!(vault.vault_key_for(&team_id).is_err());
        vault.unlock("correct horse").unwrap();
        assert!(vault.vault_key_for(&personal_id).is_ok());
        assert!(vault.vault_key_for(&team_id).is_ok());
    }

    #[test]
    fn team_vault_items_decrypt_for_every_member() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let Created {
            vault: mut vault_a, ..
        } = new_vault(&dir_a);
        let Created {
            vault: mut vault_b, ..
        } = new_vault(&dir_b);

        let team_id = "team-1".to_string();
        let team_key = SymmetricKey::generate().unwrap();
        let sealed_a =
            seal_vault_key(&vault_a.account_keys().unwrap().public(), &team_key).unwrap();
        let sealed_b =
            seal_vault_key(&vault_b.account_keys().unwrap().public(), &team_key).unwrap();
        vault_a
            .upsert_vault(&team_id, "k1", "Ops", "team", &sealed_a, "owner")
            .unwrap();
        vault_b
            .upsert_vault(&team_id, "k1", "Ops", "team", &sealed_b, "member")
            .unwrap();

        // Each member must know every member's signing key to accept the
        // other's item signatures.
        let signers = vec![
            STANDARD.encode(vault_a.account_keys().unwrap().public().sign_public),
            STANDARD.encode(vault_b.account_keys().unwrap().public().sign_public),
        ];
        vault_a.set_vault_signers(&team_id, &signers).unwrap();
        vault_b.set_vault_signers(&team_id, &signers).unwrap();

        // A writes an item into the team vault and "pushes" it (just a
        // direct transfer here, no real server).
        let id = vault_a.put_in(&team_id, None, &host("shared-web")).unwrap();
        let (env, deleted) = vault_a
            .conn
            .query_row(
                "SELECT envelope, deleted FROM items WHERE id = ?1",
                params![id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)),
            )
            .unwrap();
        let env: ItemEnvelope = serde_json::from_str(&env).unwrap();

        // B applies it as a remote update, verifying against A's key.
        assert!(vault_b.apply_remote(&env, deleted).unwrap());
        assert_eq!(vault_b.get::<Host>(&id).unwrap().name, "shared-web");

        // B edits it and "pushes" back; A must accept B's signature too.
        let mut edited = vault_b.get::<Host>(&id).unwrap();
        edited.port = 2222;
        vault_b.put_in(&team_id, Some(&id), &edited).unwrap();
        let (env2, deleted2) = vault_b
            .conn
            .query_row(
                "SELECT envelope, deleted FROM items WHERE id = ?1",
                params![id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)),
            )
            .unwrap();
        let env2: ItemEnvelope = serde_json::from_str(&env2).unwrap();
        assert!(vault_a.apply_remote(&env2, deleted2).unwrap());
        assert_eq!(vault_a.get::<Host>(&id).unwrap().port, 2222);

        // The personal vault's `list()` must not include team items, but
        // `list_all()` must.
        assert!(vault_a.list::<Host>().unwrap().is_empty());
        assert_eq!(vault_a.list_all::<Host>().unwrap().len(), 1);

        // After B is removed (forgets the key), B can no longer decrypt
        // anything in that vault.
        vault_b.forget_vault(&team_id).unwrap();
        // `forget_vault` also sweeps items under it -- nothing left to
        // even attempt decrypting.
        assert!(matches!(
            vault_b.get::<Host>(&id),
            Err(VaultError::NotFound(_))
        ));
    }

    #[test]
    fn recovery_restores_the_same_account_keys() {
        let dir = tempfile::tempdir().unwrap();
        let Created {
            mut vault,
            recovery_phrase,
        } = new_vault(&dir);
        let before = vault.header().account_public.clone();
        let id = vault.put(None, &host("x")).unwrap();
        let version_before = vault.metas().unwrap()[0].version;

        vault.lock();
        vault.recover(&recovery_phrase, "new pw").unwrap();

        // Same account keys -- no re-signing, no new version.
        assert_eq!(vault.header().account_public, before);
        assert_eq!(vault.metas().unwrap()[0].version, version_before);

        vault.lock();
        assert!(vault.unlock("correct horse").is_err());
        vault.unlock("new pw").unwrap();
        assert_eq!(vault.get::<Host>(&id).unwrap().name, "x");
    }
}
