// TypeScript mirror of crates/proto/src/{rpc,agent}.rs and sync.rs.
// Keep in sync with the Rust source: it is the single source of truth for the
// wire protocol. Binary fields (PTY bytes, file contents) travel as base64
// strings here (the agent base64-encodes them per docs/agent-protocol.md), not
// as Vec<u8> like on the Rust side.

// ----- JSON-RPC framing (crates/proto/src/rpc.rs) -----

export const JSONRPC = "2.0";

export const ErrorCode = {
  PARSE_ERROR: -32700,
  INVALID_REQUEST: -32600,
  METHOD_NOT_FOUND: -32601,
  INVALID_PARAMS: -32602,
  INTERNAL_ERROR: -32603,
  VAULT_LOCKED: -32001,
  NOT_FOUND: -32002,
  POLICY_DENIED: -32003,
  APPROVAL_REJECTED: -32004,
  APPROVAL_TIMEOUT: -32005,
  HOST_KEY_UNKNOWN: -32006,
  HOST_KEY_MISMATCH: -32007,
  HANDSHAKE_REQUIRED: -32008,
} as const;

export interface RpcError {
  code: number;
  message: string;
  data?: unknown;
}

export class AgentRpcError extends Error {
  code: number;
  data?: unknown;
  constructor(err: RpcError) {
    super(err.message);
    this.name = "AgentRpcError";
    this.code = err.code;
    this.data = err.data;
  }
}

// ----- Agent methods (crates/proto/src/agent.rs) -----

export const Method = {
  HELLO: "hello",
  VAULT_STATUS: "vault.status",
  VAULT_INIT: "vault.init",
  VAULT_UNLOCK: "vault.unlock",
  VAULT_LOCK: "vault.lock",
  ITEM_LIST: "item.list",
  ITEM_GET: "item.get",
  ITEM_PUT: "item.put",
  ITEM_DELETE: "item.delete",
  KEY_GENERATE: "key.generate",
  KEY_IMPORT: "key.import",
  IMPORT_SSH_CONFIG: "import.ssh_config",
  IMPORT_TERMIUS_CSV: "import.termius_csv",
  PTY_OPEN: "pty.open",
  PTY_WRITE: "pty.write",
  PTY_RESIZE: "pty.resize",
  PTY_CLOSE: "pty.close",
  EXEC_RUN: "exec.run",
  SFTP_LIST: "sftp.list",
  SFTP_READ: "sftp.read",
  SFTP_WRITE: "sftp.write",
  FORWARD_OPEN: "forward.open",
  FORWARD_LIST: "forward.list",
  FORWARD_CLOSE: "forward.close",
  HOSTKEY_TRUST: "hostkey.trust",
  AI_HOSTS: "ai.hosts",
  AI_RUN: "ai.run",
  AI_LIST_DIR: "ai.list_dir",
  AI_READ_FILE: "ai.read_file",
  AI_WRITE_FILE: "ai.write_file",
  AI_APPROVALS: "ai.approvals",
  AI_DECIDE: "ai.decide",
  AI_DANGEROUS: "ai.dangerous",
  AI_STOP: "ai.stop",
  AI_STATUS: "ai.status",
  AI_AUDIT: "ai.audit",
  EVENTS_SUBSCRIBE: "events.subscribe",
  SYNC_RUN: "sync.run",
  SHUTDOWN: "agent.shutdown",
} as const;

export const EventMethod = {
  PTY_DATA: "pty.data",
  PTY_EXIT: "pty.exit",
  VAULT_LOCKED: "vault.locked",
  VAULT_UNLOCKED: "vault.unlocked",
  APPROVAL_REQUEST: "approval.request",
  APPROVAL_RESOLVED: "approval.resolved",
  AI_MODE: "ai.mode",
  AI_CALL: "ai.call",
} as const;

export type ClientKind = "cli" | "desktop" | "mcp";

export interface HelloParams {
  protocol: number;
  client: ClientKind;
  ai_session?: string | null;
  ai_client_name?: string | null;
}

export interface HelloResult {
  protocol: number;
  agent_version: string;
}

export interface VaultStatus {
  exists: boolean;
  unlocked: boolean;
  path: string;
  local_only: boolean;
  auto_lock_in: number | null;
}

export interface VaultInitParams {
  password: string;
  local_only?: boolean;
}

export interface VaultInitResult {
  recovery_phrase: string;
}

export interface UnlockParams {
  password: string;
  auto_lock_secs?: number | null;
}

/// Item kinds exposed over RPC.
export type ItemKind =
  | "host"
  | "identity"
  | "key"
  | "snippet"
  | "group"
  | "known_host"
  | "audit";

export interface ItemListParams {
  kind: ItemKind;
}

export interface ItemRecord<T = unknown> {
  id: string;
  kind: ItemKind;
  item: T;
}

export interface ItemRef {
  kind: ItemKind;
  /** Id or name. */
  id: string;
}

export interface ItemPutParams<T = unknown> {
  kind: ItemKind;
  id?: string | null;
  item: T;
}

// ----- Plaintext item shapes (crates/core/src/vault/model.rs) -----
// These travel inside ItemRecord.item / ItemPutParams.item as plain JSON.

export interface Host {
  name: string;
  hostname: string;
  port: number;
  username: string | null;
  identity_id: string | null;
  group_id: string | null;
  tags: string[];
  jump_host: string | null;
  ai_allowed: boolean;
  ai_user: string | null;
  notes: string | null;
}

export interface Identity {
  name: string;
  username: string | null;
  password: string | null;
  key_id: string | null;
}

export interface SshKey {
  name: string;
  private_key: string;
  public_key: string;
  passphrase: string | null;
}

export interface Snippet {
  name: string;
  script: string;
}

export interface Group {
  name: string;
  parent_id: string | null;
}

export interface KnownHost {
  /** `host:port` */
  host: string;
  key: string;
}

export interface AuditEntry {
  time: string;
  ai_session: string;
  host: string;
  tool: string;
  detail: string;
  mode: string;
  approver: string;
  outcome: string;
  exit_code: number | null;
}

export function emptyHost(): Host {
  return {
    name: "",
    hostname: "",
    port: 22,
    username: null,
    identity_id: null,
    group_id: null,
    tags: [],
    jump_host: null,
    ai_allowed: false,
    ai_user: null,
    notes: null,
  };
}

// ----- Key tools -----

export type KeyAlgorithm = "ed25519" | "rsa";

export interface KeyGenerateParams {
  name: string;
  algorithm: KeyAlgorithm;
  bits?: number | null;
  passphrase?: string | null;
}

export interface KeyImportParams {
  name: string;
  private_key: string;
  passphrase?: string | null;
}

// ----- Import -----

export interface ImportFileParams {
  /** File contents (not a path): the agent never reads arbitrary paths for a client. */
  contents: string;
  base_dir?: string | null;
}

// ----- Sessions / PTY -----

export interface HostTarget {
  /** Host id or name. */
  host: string;
  /** Accept an unknown host key (after the user confirmed its fingerprint). */
  accept_host_key?: string | null;
}

export interface PtyOpenParams extends HostTarget {
  term?: string;
  cols: number;
  rows: number;
  agent_forwarding?: boolean;
}

export interface PtyOpenResult {
  session_id: string;
}

export interface PtyWriteParams {
  session_id: string;
  /** base64 */
  data: string;
}

export interface PtyResizeParams {
  session_id: string;
  cols: number;
  rows: number;
}

export interface SessionRef {
  session_id: string;
}

export interface PtyDataEvent {
  session_id: string;
  /** base64 */
  data: string;
}

export interface PtyExitEvent {
  session_id: string;
  exit_code: number | null;
}

export interface ExecParams extends HostTarget {
  command: string;
  timeout_secs?: number | null;
  max_output?: number | null;
}

export interface ExecResult {
  stdout: string;
  stderr: string;
  exit_code: number | null;
  truncated: boolean;
}

// ----- SFTP -----

export interface SftpPathParams extends HostTarget {
  path: string;
  max_bytes?: number | null;
}

export interface SftpWriteParams extends HostTarget {
  path: string;
  /** base64 */
  data: string;
}

export interface DirEntry {
  name: string;
  is_dir: boolean;
  size: number | null;
  modified: number | null;
}

export interface FileData {
  /** base64 */
  data: string;
  truncated: boolean;
}

// ----- Port forwarding -----

export type ForwardKind = "local" | "remote";

export interface ForwardOpenParams extends HostTarget {
  kind: ForwardKind;
  bind: string;
  dest: string;
}

export interface ForwardInfo {
  id: string;
  host: string;
  kind: ForwardKind;
  bind: string;
  dest: string;
}

// ----- AI -----

export interface AiHost {
  name: string;
  tags: string[];
}

export interface ApprovalRequest {
  approval_id: string;
  ai_session: string;
  ai_client_name: string | null;
  host: string;
  tool: string;
  detail: string;
  created_at: string;
}

export interface DecideParams {
  approval_id: string;
  approve: boolean;
}

export interface DangerousParams {
  ai_session: string;
  hosts: string[];
  /** Minutes until dangerous mode ends (default 60). `0` turns it off. */
  minutes?: number | null;
}

export interface AiSessionInfo {
  ai_session: string;
  ai_client_name: string | null;
  dangerous_hosts: string[];
  dangerous_until: string | null;
}

export interface AiStatus {
  sessions: AiSessionInfo[];
  pending: ApprovalRequest[];
}

export interface AuditQuery {
  limit?: number | null;
}

export interface SyncParams {
  server?: string | null;
}

// ----- Agent events (payload shapes for EventMethod.*) -----

export interface VaultLockedEvent {
  reason?: string;
}

export interface AiModeEvent {
  ai_session: string;
  dangerous_hosts: string[];
  dangerous_until: string | null;
}

export interface AgentEventPayload<T = unknown> {
  method: string;
  params: T;
}

export interface AgentStatusPayload {
  connected: boolean;
  detail: string;
}
