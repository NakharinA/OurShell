import { describe, expect, it, vi, beforeEach } from "vitest";

// `@tauri-apps/api/*` talk to `window.__TAURI_INTERNALS__`, which doesn't exist
// outside a real Tauri webview. Mock the two entry points `rpc.ts` uses so the
// RPC client and event bus can be tested without one. `vi.mock` factories are
// hoisted above the rest of the file, so the mocks themselves must be built
// with `vi.hoisted`.
const { invokeMock, listenMock, listeners } = vi.hoisted(() => {
  const listeners = new Map<string, Set<(event: { payload: unknown }) => void>>();
  const invokeMock = vi.fn();
  const listenMock = vi.fn(async (event: string, handler: (e: { payload: unknown }) => void) => {
    let set = listeners.get(event);
    if (!set) {
      set = new Set();
      listeners.set(event, set);
    }
    set.add(handler);
    return () => set!.delete(handler);
  });
  return { invokeMock, listenMock, listeners };
});

vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));

function emit(event: string, payload: unknown): void {
  for (const handler of listeners.get(event) ?? []) handler({ payload });
}

import { AgentRpcError } from "./types";
import { base64ToBytes, base64ToText, bytesToBase64, rpc, textToBase64 } from "./rpc";

beforeEach(() => {
  invokeMock.mockReset();
  listenMock.mockClear();
  listeners.clear();
  rpc.dispose();
});

describe("base64 helpers", () => {
  it("round-trips arbitrary bytes", () => {
    const bytes = new Uint8Array([0, 1, 2, 250, 251, 252, 253, 254, 255]);
    expect(base64ToBytes(bytesToBase64(bytes))).toEqual(bytes);
  });

  it("round-trips buffers larger than the chunk size (32KiB)", () => {
    const bytes = new Uint8Array(0x8000 * 3 + 7);
    for (let i = 0; i < bytes.length; i++) bytes[i] = i % 256;
    const decoded = base64ToBytes(bytesToBase64(bytes));
    expect(decoded).toEqual(bytes);
  });

  it("round-trips UTF-8 text, including multi-byte characters", () => {
    const text = "hello éè 世界 😀";
    expect(base64ToText(textToBase64(text))).toBe(text);
  });

  it("matches the base64 the agent would send for PTY data", () => {
    // "hi\n" -> known base64, per docs/agent-protocol.md's base64 framing.
    expect(bytesToBase64(new TextEncoder().encode("hi\n"))).toBe("aGkK");
    expect(base64ToText("aGkK")).toBe("hi\n");
  });
});

describe("AgentClient.call", () => {
  it("resolves with the agent's result", async () => {
    invokeMock.mockResolvedValue({ exists: true, unlocked: false });
    const result = await rpc.call("vault.status");
    expect(invokeMock).toHaveBeenCalledWith("rpc_call", { method: "vault.status", params: {} });
    expect(result).toEqual({ exists: true, unlocked: false });
  });

  it("passes params through untouched", async () => {
    invokeMock.mockResolvedValue(null);
    await rpc.call("vault.unlock", { password: "secret" });
    expect(invokeMock).toHaveBeenCalledWith("rpc_call", {
      method: "vault.unlock",
      params: { password: "secret" },
    });
  });

  it("wraps an agent RpcError rejection in an AgentRpcError", async () => {
    invokeMock.mockRejectedValue({
      code: -32001,
      message: "vault is locked",
      data: null,
    });
    await expect(rpc.call("item.list", { kind: "host" })).rejects.toMatchObject({
      name: "AgentRpcError",
      code: -32001,
      message: "vault is locked",
    });
    await expect(rpc.call("item.list", { kind: "host" })).rejects.toBeInstanceOf(AgentRpcError);
  });

  it("carries the error's `data` through (e.g. a host-key fingerprint)", async () => {
    invokeMock.mockRejectedValue({
      code: -32006,
      message: "unknown host key",
      data: "SHA256:abc123",
    });
    try {
      await rpc.call("pty.open", { host: "prod" });
      expect.unreachable("should have thrown");
    } catch (err) {
      expect(err).toBeInstanceOf(AgentRpcError);
      expect((err as AgentRpcError).data).toBe("SHA256:abc123");
    }
  });

  it("passes through a rejection that isn't RpcError-shaped", async () => {
    invokeMock.mockRejectedValue(new Error("tauri transport failure"));
    await expect(rpc.call("vault.status")).rejects.toThrow("tauri transport failure");
  });
});

describe("AgentClient event routing", () => {
  it("delivers agent-event payloads to onEvent subscribers", async () => {
    await rpc.init();
    const seen: Array<{ method: string; params: unknown }> = [];
    const unsub = rpc.onEvent((method, params) => seen.push({ method, params }));

    emit("agent-event", { method: "pty.data", params: { session_id: "s1", data: "aGk=" } });
    emit("agent-event", { method: "vault.locked", params: {} });

    expect(seen).toEqual([
      { method: "pty.data", params: { session_id: "s1", data: "aGk=" } },
      { method: "vault.locked", params: {} },
    ]);

    unsub();
    emit("agent-event", { method: "pty.data", params: {} });
    expect(seen).toHaveLength(2); // unsubscribed handler sees nothing further
  });

  it("tracks agent-status and notifies onStatus subscribers", async () => {
    await rpc.init();
    expect(rpc.getLastStatus()).toEqual({ connected: false, detail: "starting" });

    const statuses: Array<{ connected: boolean; detail: string }> = [];
    rpc.onStatus((s) => statuses.push(s));

    emit("agent-status", { connected: true, detail: "connected" });
    expect(rpc.getLastStatus()).toEqual({ connected: true, detail: "connected" });
    expect(statuses).toEqual([{ connected: true, detail: "connected" }]);
  });

  it("init() only registers its listeners once", async () => {
    await rpc.init();
    await rpc.init();
    expect(listenMock).toHaveBeenCalledTimes(2); // agent-event + agent-status, not 4
  });
});
