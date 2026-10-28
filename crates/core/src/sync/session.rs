//! Persisted sync session state.
//!
//! Stored encrypted *inside* the vault's own SQLite file via
//! [`crate::vault::Vault::set_secret_meta`] -- never synced to the server,
//! and never written in plain text (see `docs/self-hosting.md`'s
//! "Connecting clients" section for the deliberate deviation this is from
//! the project plan's "OS keychain" suggestion: it keeps the client
//! dependency-free while still encrypting the token at rest).

use std::collections::HashMap;

use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};

use crate::vault::Vault;

use super::error::Result;

const META_KEY: &str = "sync_session";

/// Everything needed to talk to a sync server as one already-registered
/// device, plus a pull cursor *per vault* (personal and every team vault
/// this account belongs to) so `sync()` can resume each one incrementally.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncSession {
    pub server_url: String,
    pub email: String,
    pub device_id: String,
    pub device_secret: [u8; 32],
    pub session_token: String,
    /// `vault_id -> cursor`. Replaces the single flat `cursor` field a
    /// personal-vault-only session used to have; an old session missing
    /// this (or with an unrecognized vault id) simply resumes that vault
    /// from the start, which is safe (pull/apply is idempotent).
    #[serde(default)]
    pub cursors: HashMap<String, i64>,
    #[serde(default)]
    pub last_sync: Option<String>,
}

impl SyncSession {
    pub fn cursor_for(&self, vault_id: &str) -> i64 {
        self.cursors.get(vault_id).copied().unwrap_or(0)
    }

    pub fn set_cursor(&mut self, vault_id: &str, cursor: i64) {
        self.cursors.insert(vault_id.to_string(), cursor);
    }
}

impl SyncSession {
    /// This device's Ed25519 identity (registered with the server at
    /// signup/login time; not otherwise used in today's protocol, but kept
    /// so re-logins reuse the same device record instead of growing the
    /// account's device list).
    pub fn signing_key(&self) -> SigningKey {
        SigningKey::from_bytes(&self.device_secret)
    }

    /// Loads the session persisted in `vault`, if any. The vault must be
    /// unlocked.
    pub fn load(vault: &Vault) -> Result<Option<Self>> {
        match vault.get_secret_meta(META_KEY)? {
            Some(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            None => Ok(None),
        }
    }

    /// Persists this session into `vault`, replacing any previous one.
    pub fn save(&self, vault: &mut Vault) -> Result<()> {
        vault.set_secret_meta(META_KEY, &serde_json::to_vec(self)?)?;
        Ok(())
    }

    /// Removes any session persisted in `vault` (used by `logout`).
    pub fn clear(vault: &mut Vault) -> Result<()> {
        vault.delete_secret_meta(META_KEY)?;
        Ok(())
    }
}
