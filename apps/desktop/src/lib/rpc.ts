// Typed JSON-RPC client talking to the `ourshell agent` over the Rust bridge.
//
// The desktop app never does crypto or SSH itself. Every call here goes through
// the Tauri command `rpc_call(method, params)`, which the Rust side
// (src-tauri/src/agent.rs) forwards verbatim over the agent's local socket as a
// JSON-RPC 2.0 request, per docs/agent-protocol.md. Agent notifications (PTY
// data, approval requests, lock events, ...) arrive as a Tauri event named
// `agent-event` with `{ method, params }`; connectivity changes arrive as
// `agent-status` with `{ connected, detail }`. See `src/lib/stores/events.ts`
// for the event bus built on top of these.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  AgentRpcError,
  Method,
  type AgentStatusPayload,
  type AiStatus,
  type AuditEntry,
  type AuditQuery,
  type DangerousParams,
  type DecideParams,
  type ExecParams,
  type ExecResult,
  type ForwardInfo,
  type ForwardOpenParams,
  type HelloResult,
  type HostTarget,
  type ImportFileParams,
  type ItemKind,
  type ItemPutParams,
  type ItemRecord,
  type ItemRef,
  type KeyGenerateParams,
  type KeyImportParams,
  type PtyOpenParams,
  type PtyOpenResult,
  type PtyResizeParams,
  type RpcError,
  type SftpPathParams,
  type SftpWriteParams,
  type DirEntry,
  type FileData,
  type SessionRef,
  type UnlockParams,
  type VaultInitParams,
  type VaultInitResult,
  type VaultStatus,
  type ImportReport,
  type KeyResult,
} from "./types";

// ----- base64 helpers (PTY bytes / file contents travel as base64 strings) -----

export function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

export function base64ToBytes(b64: string): Uint8Array {
  const binary = atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

export function textToBase64(text: string): string {
  return bytesToBase64(new TextEncoder().encode(text));
}

export function base64ToText(b64: string): string {
  return new TextDecoder().decode(base64ToBytes(b64));
}

// ----- low-level call + event plumbing -----

export type AgentEventHandler = (method: string, params: unknown) => void;
export type AgentStatusHandler = (status: AgentStatusPayload) => void;

class AgentClient {
  private eventHandlers = new Set<AgentEventHandler>();
  private statusHandlers = new Set<AgentStatusHandler>();
  private unlistenEvent: UnlistenFn | null = null;
  private unlistenStatus: UnlistenFn | null = null;
  private lastStatus: AgentStatusPayload = { connected: false, detail: "starting" };

  /** Starts listening for `agent-event` / `agent-status`. Call once at app startup. */
  async init(): Promise<void> {
    if (this.unlistenEvent) return;
    this.unlistenEvent = await listen<{ method: string; params: unknown }>(
      "agent-event",
      (e) => {
        for (const h of this.eventHandlers) h(e.payload.method, e.payload.params);
      },
    );
    this.unlistenStatus = await listen<AgentStatusPayload>("agent-status", (e) => {
      this.lastStatus = e.payload;
      for (const h of this.statusHandlers) h(e.payload);
    });
  }

  dispose(): void {
    this.unlistenEvent?.();
    this.unlistenStatus?.();
    this.unlistenEvent = null;
    this.unlistenStatus = null;
    this.eventHandlers.clear();
    this.statusHandlers.clear();
  }

  getLastStatus(): AgentStatusPayload {
    return this.lastStatus;
  }

  onEvent(handler: AgentEventHandler): () => void {
    this.eventHandlers.add(handler);
    return () => this.eventHandlers.delete(handler);
  }

  onStatus(handler: AgentStatusHandler): () => void {
    this.statusHandlers.add(handler);
    return () => this.statusHandlers.delete(handler);
  }

  /** Raw call: any method name, any params, the agent's `result` typed as `R`. */
  async call<R = unknown>(method: string, params: unknown = {}): Promise<R> {
    try {
      return (await invoke("rpc_call", { method, params })) as R;
    } catch (err) {
      throw toAgentError(err);
    }
  }

  // ----- vault -----

  vaultStatus(): Promise<VaultStatus> {
    return this.call(Method.VAULT_STATUS);
  }
  vaultInit(params: VaultInitParams): Promise<VaultInitResult> {
    return this.call(Method.VAULT_INIT, params);
  }
  vaultUnlock(params: UnlockParams): Promise<void> {
    return this.call(Method.VAULT_UNLOCK, params);
  }
  vaultLock(): Promise<void> {
    return this.call(Method.VAULT_LOCK);
  }

  // ----- items (host / identity / key / snippet / group / known_host / audit) -----

  itemList<T = unknown>(kind: ItemKind): Promise<ItemRecord<T>[]> {
    return this.call(Method.ITEM_LIST, { kind });
  }
  itemGet<T = unknown>(ref: ItemRef): Promise<ItemRecord<T>> {
    return this.call(Method.ITEM_GET, ref);
  }
  itemPut<T = unknown>(params: ItemPutParams<T>): Promise<ItemRecord<T>> {
    return this.call(Method.ITEM_PUT, params);
  }
  itemDelete(ref: ItemRef): Promise<void> {
    return this.call(Method.ITEM_DELETE, ref);
  }

  // ----- keys -----

  keyGenerate(params: KeyGenerateParams): Promise<KeyResult> {
    return this.call(Method.KEY_GENERATE, params);
  }
  keyImport(params: KeyImportParams): Promise<KeyResult> {
    return this.call(Method.KEY_IMPORT, params);
  }

  // ----- import -----

  importSshConfig(params: ImportFileParams): Promise<ImportReport> {
    return this.call(Method.IMPORT_SSH_CONFIG, params);
  }
  importTermiusCsv(params: ImportFileParams): Promise<ImportReport> {
    return this.call(Method.IMPORT_TERMIUS_CSV, params);
  }

  // ----- pty -----

  ptyOpen(params: PtyOpenParams): Promise<PtyOpenResult> {
    return this.call(Method.PTY_OPEN, params);
  }
  ptyWrite(sessionId: string, data: Uint8Array): Promise<void> {
    return this.call(Method.PTY_WRITE, {
      session_id: sessionId,
      data: bytesToBase64(data),
    });
  }
  ptyResize(params: PtyResizeParams): Promise<void> {
    return this.call(Method.PTY_RESIZE, params);
  }
  ptyClose(ref: SessionRef): Promise<void> {
    return this.call(Method.PTY_CLOSE, ref);
  }

  // ----- exec -----

  execRun(params: ExecParams): Promise<ExecResult> {
    return this.call(Method.EXEC_RUN, params);
  }

  // ----- sftp -----

  sftpList(params: SftpPathParams): Promise<DirEntry[]> {
    return this.call(Method.SFTP_LIST, params);
  }
  sftpRead(params: SftpPathParams): Promise<FileData> {
    return this.call(Method.SFTP_READ, params);
  }
  sftpWrite(params: SftpWriteParams): Promise<void> {
    return this.call(Method.SFTP_WRITE, params);
  }

  // ----- forwarding -----

  forwardOpen(params: ForwardOpenParams): Promise<ForwardInfo> {
    return this.call(Method.FORWARD_OPEN, params);
  }
  forwardList(): Promise<ForwardInfo[]> {
    return this.call(Method.FORWARD_LIST);
  }
  forwardClose(id: string): Promise<void> {
    return this.call(Method.FORWARD_CLOSE, { id });
  }

  // ----- host keys -----

  hostkeyTrust(target: HostTarget): Promise<void> {
    return this.call(Method.HOSTKEY_TRUST, target);
  }

  // ----- AI -----

  aiHosts(): Promise<unknown> {
    return this.call(Method.AI_HOSTS);
  }
  aiApprovals(): Promise<unknown> {
    return this.call(Method.AI_APPROVALS);
  }
  aiDecide(params: DecideParams): Promise<void> {
    return this.call(Method.AI_DECIDE, params);
  }
  aiDangerous(params: DangerousParams): Promise<void> {
    return this.call(Method.AI_DANGEROUS, params);
  }
  aiStop(): Promise<void> {
    return this.call(Method.AI_STOP);
  }
  aiStatus(): Promise<AiStatus> {
    return this.call(Method.AI_STATUS);
  }
  aiAudit(query: AuditQuery = {}): Promise<AuditEntry[]> {
    return this.call(Method.AI_AUDIT, query);
  }

  // ----- misc -----

  hello(): Promise<HelloResult> {
    return this.call(Method.HELLO);
  }
}

function toAgentError(err: unknown): AgentRpcError | unknown {
  // Tauri surfaces a `Result::Err` from a `#[tauri::command]` as the rejection
  // value itself (our `RpcError` serialized as plain JSON), not wrapped.
  if (isRpcErrorShape(err)) {
    return new AgentRpcError(err);
  }
  return err;
}

function isRpcErrorShape(v: unknown): v is RpcError {
  return (
    typeof v === "object" &&
    v !== null &&
    "code" in v &&
    typeof (v as { code: unknown }).code === "number" &&
    "message" in v &&
    typeof (v as { message: unknown }).message === "string"
  );
}

/** Singleton RPC client for the whole app. */
export const rpc = new AgentClient();
