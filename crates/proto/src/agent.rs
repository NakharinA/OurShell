//! Agent method names and their parameter / result types.
//!
//! Vault items travel as JSON objects (`serde_json::Value`) so this crate does not
//! depend on `ourshell-core`; the agent serializes `ourshell_core::vault` types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod method {
    pub const HELLO: &str = "hello";
    pub const VAULT_STATUS: &str = "vault.status";
    pub const VAULT_INIT: &str = "vault.init";
    pub const VAULT_UNLOCK: &str = "vault.unlock";
    pub const VAULT_LOCK: &str = "vault.lock";
    pub const ITEM_LIST: &str = "item.list";
    pub const ITEM_GET: &str = "item.get";
    pub const ITEM_PUT: &str = "item.put";
    pub const ITEM_DELETE: &str = "item.delete";
    pub const KEY_GENERATE: &str = "key.generate";
    pub const KEY_IMPORT: &str = "key.import";
    pub const IMPORT_SSH_CONFIG: &str = "import.ssh_config";
    pub const IMPORT_TERMIUS_CSV: &str = "import.termius_csv";
    pub const PTY_OPEN: &str = "pty.open";
    pub const PTY_WRITE: &str = "pty.write";
    pub const PTY_RESIZE: &str = "pty.resize";
    pub const PTY_CLOSE: &str = "pty.close";
    pub const EXEC_RUN: &str = "exec.run";
    pub const SFTP_LIST: &str = "sftp.list";
    pub const SFTP_READ: &str = "sftp.read";
    pub const SFTP_WRITE: &str = "sftp.write";
    pub const FORWARD_OPEN: &str = "forward.open";
    pub const FORWARD_LIST: &str = "forward.list";
    pub const FORWARD_CLOSE: &str = "forward.close";
    pub const HOSTKEY_TRUST: &str = "hostkey.trust";
    pub const AI_HOSTS: &str = "ai.hosts";
    pub const AI_RUN: &str = "ai.run";
    pub const AI_LIST_DIR: &str = "ai.list_dir";
    pub const AI_READ_FILE: &str = "ai.read_file";
    pub const AI_WRITE_FILE: &str = "ai.write_file";
    pub const AI_APPROVALS: &str = "ai.approvals";
    pub const AI_DECIDE: &str = "ai.decide";
    pub const AI_DANGEROUS: &str = "ai.dangerous";
    pub const AI_STOP: &str = "ai.stop";
    pub const AI_STATUS: &str = "ai.status";
    pub const AI_AUDIT: &str = "ai.audit";
    pub const EVENTS_SUBSCRIBE: &str = "events.subscribe";
    pub const SYNC_RUN: &str = "sync.run";
    pub const SHUTDOWN: &str = "agent.shutdown";
}

pub mod event {
    pub const PTY_DATA: &str = "pty.data";
    pub const PTY_EXIT: &str = "pty.exit";
    pub const VAULT_LOCKED: &str = "vault.locked";
    pub const VAULT_UNLOCKED: &str = "vault.unlocked";
    pub const APPROVAL_REQUEST: &str = "approval.request";
    pub const APPROVAL_RESOLVED: &str = "approval.resolved";
    pub const AI_MODE: &str = "ai.mode";
    pub const AI_CALL: &str = "ai.call";
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClientKind {
    Cli,
    Desktop,
    Mcp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloParams {
    pub protocol: u32,
    pub client: ClientKind,
    /// Required for MCP clients; identifies one AI session.
    #[serde(default)]
    pub ai_session: Option<String>,
    /// Optional human-readable label, e.g. "Claude Code".
    #[serde(default)]
    pub ai_client_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloResult {
    pub protocol: u32,
    pub agent_version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultStatus {
    pub exists: bool,
    pub unlocked: bool,
    pub path: String,
    pub local_only: bool,
    /// Seconds until auto-lock, if unlocked.
    pub auto_lock_in: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VaultInitParams {
    pub password: String,
    #[serde(default)]
    pub local_only: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VaultInitResult {
    pub recovery_phrase: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnlockParams {
    pub password: String,
    /// Auto-lock after this many idle seconds (default 15 minutes).
    #[serde(default)]
    pub auto_lock_secs: Option<u64>,
}

/// Item kinds exposed over RPC: host, identity, key, snippet, group, known_host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemListParams {
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ItemRecord {
    pub id: String,
    pub kind: String,
    pub item: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemRef {
    pub kind: String,
    /// Id or name.
    pub id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ItemPutParams {
    pub kind: String,
    #[serde(default)]
    pub id: Option<String>,
    pub item: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyGenerateParams {
    pub name: String,
    /// `ed25519` or `rsa`.
    pub algorithm: String,
    #[serde(default)]
    pub bits: Option<usize>,
    #[serde(default)]
    pub passphrase: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyImportParams {
    pub name: String,
    pub private_key: String,
    #[serde(default)]
    pub passphrase: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportFileParams {
    /// File contents (not a path), so the agent never reads arbitrary paths for a client.
    pub contents: String,
    /// Directory used to resolve `IdentityFile` / `Include` (ssh_config only).
    #[serde(default)]
    pub base_dir: Option<String>,
}

/// Which host to act on, plus an optional password/passphrase prompt answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostTarget {
    /// Host id or name.
    pub host: String,
    /// Accept an unknown host key (after the user confirmed its fingerprint).
    #[serde(default)]
    pub accept_host_key: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyOpenParams {
    #[serde(flatten)]
    pub target: HostTarget,
    #[serde(default = "default_term")]
    pub term: String,
    pub cols: u32,
    pub rows: u32,
    #[serde(default)]
    pub agent_forwarding: bool,
}

fn default_term() -> String {
    "xterm-256color".into()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyOpenResult {
    pub session_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyWriteParams {
    pub session_id: String,
    #[serde(with = "crate::b64")]
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyResizeParams {
    pub session_id: String,
    pub cols: u32,
    pub rows: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRef {
    pub session_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyDataEvent {
    pub session_id: String,
    #[serde(with = "crate::b64")]
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyExitEvent {
    pub session_id: String,
    pub exit_code: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecParams {
    #[serde(flatten)]
    pub target: HostTarget,
    pub command: String,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub max_output: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<u32>,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SftpPathParams {
    #[serde(flatten)]
    pub target: HostTarget,
    pub path: String,
    #[serde(default)]
    pub max_bytes: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SftpWriteParams {
    #[serde(flatten)]
    pub target: HostTarget,
    pub path: String,
    #[serde(with = "crate::b64")]
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub modified: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileData {
    #[serde(with = "crate::b64")]
    pub data: Vec<u8>,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ForwardKind {
    Local,
    Remote,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardOpenParams {
    #[serde(flatten)]
    pub target: HostTarget,
    pub kind: ForwardKind,
    /// Local: local bind address (e.g. `127.0.0.1:8080`). Remote: remote bind address.
    pub bind: String,
    /// Local: remote destination `host:port`. Remote: local destination `host:port`.
    pub dest: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardInfo {
    pub id: String,
    pub host: String,
    pub kind: ForwardKind,
    pub bind: String,
    pub dest: String,
}

// ----- AI -----

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiHost {
    pub name: String,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub approval_id: String,
    pub ai_session: String,
    pub ai_client_name: Option<String>,
    pub host: String,
    pub tool: String,
    /// Command, path, or a short description of the write.
    pub detail: String,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecideParams {
    pub approval_id: String,
    pub approve: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DangerousParams {
    pub ai_session: String,
    /// Host names to allow without approval; must be AI-allowed hosts.
    pub hosts: Vec<String>,
    /// Minutes until dangerous mode ends (default 60). `0` turns it off.
    #[serde(default)]
    pub minutes: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiSessionInfo {
    pub ai_session: String,
    pub ai_client_name: Option<String>,
    pub dangerous_hosts: Vec<String>,
    pub dangerous_until: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiStatus {
    pub sessions: Vec<AiSessionInfo>,
    pub pending: Vec<ApprovalRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditQuery {
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncParams {
    #[serde(default)]
    pub server: Option<String>,
}
