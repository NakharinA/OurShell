//! Plaintext item types. These are serialized to JSON and only ever stored encrypted.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// A vault item type with a stable type tag used in the envelope.
pub trait Item: Serialize + DeserializeOwned {
    const KIND: &'static str;
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Host {
    /// Display name / alias, unique within the vault (e.g. `prod-web`).
    pub name: String,
    pub hostname: String,
    pub port: u16,
    pub username: Option<String>,
    /// Id of an [`Identity`] to authenticate with.
    pub identity_id: Option<String>,
    pub group_id: Option<String>,
    pub tags: Vec<String>,
    /// Name or id of another host to jump through (ProxyJump).
    pub jump_host: Option<String>,
    /// Hidden from AI tools unless true.
    pub ai_allowed: bool,
    /// Optional user override for AI-initiated sessions (e.g. a restricted `ai` user).
    pub ai_user: Option<String>,
    pub notes: Option<String>,
}

impl Host {
    pub fn port_or_default(&self) -> u16 {
        if self.port == 0 {
            22
        } else {
            self.port
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Identity {
    pub name: String,
    pub username: Option<String>,
    pub password: Option<String>,
    /// Id of an [`SshKey`].
    pub key_id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SshKey {
    pub name: String,
    /// OpenSSH-format private key (may itself be passphrase-protected).
    pub private_key: String,
    pub public_key: String,
    pub passphrase: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Snippet {
    pub name: String,
    pub script: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Group {
    pub name: String,
    pub parent_id: Option<String>,
}

/// A known-hosts entry trusted by the user (stored encrypted in the vault).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KnownHost {
    /// `host:port`
    pub host: String,
    /// OpenSSH public key line, e.g. `ssh-ed25519 AAAA...`.
    pub key: String,
}

/// One AI tool call, recorded for the audit log.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AuditEntry {
    pub time: String,
    pub ai_session: String,
    pub host: String,
    pub tool: String,
    pub detail: String,
    pub mode: String,
    pub approver: String,
    pub outcome: String,
    pub exit_code: Option<i32>,
}

impl Item for Host {
    const KIND: &'static str = "host";
}
impl Item for Identity {
    const KIND: &'static str = "identity";
}
impl Item for SshKey {
    const KIND: &'static str = "key";
}
impl Item for Snippet {
    const KIND: &'static str = "snippet";
}
impl Item for Group {
    const KIND: &'static str = "group";
}
impl Item for KnownHost {
    const KIND: &'static str = "known_host";
}
impl Item for AuditEntry {
    const KIND: &'static str = "audit";
}
