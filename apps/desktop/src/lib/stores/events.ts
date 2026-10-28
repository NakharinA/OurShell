// Event bus over the agent's notifications (`agent-event` / `agent-status`,
// forwarded by src-tauri/src/agent.rs). Components subscribe to one `method`
// (e.g. `pty.data`, `approval.request`) instead of every component re-parsing
// the raw Tauri event.

import { writable } from "svelte/store";
import { rpc } from "../rpc";
import type { AgentStatusPayload } from "../types";
import { EventMethod } from "../types";

export const agentStatus = writable<AgentStatusPayload>({
  connected: false,
  detail: "starting",
});

/** Bumped whenever a `vault.locked` event arrives, so the unlock screen can show. */
export const vaultLockedTick = writable(0);

type Listener = (params: unknown) => void;
const byMethod = new Map<string, Set<Listener>>();

/** Subscribe to one agent notification method. Returns an unsubscribe function. */
export function onAgentEvent(method: string, handler: Listener): () => void {
  let set = byMethod.get(method);
  if (!set) {
    set = new Set();
    byMethod.set(method, set);
  }
  set.add(handler);
  return () => set!.delete(handler);
}

let started = false;

/** Wires the RPC client's event stream into the bus. Call once at startup. */
export async function startEventBus(): Promise<void> {
  if (started) return;
  started = true;
  await rpc.init();

  agentStatus.set(rpc.getLastStatus());
  rpc.onStatus((s) => agentStatus.set(s));

  rpc.onEvent((method, params) => {
    if (method === EventMethod.VAULT_LOCKED) {
      vaultLockedTick.update((n) => n + 1);
    }
    const set = byMethod.get(method);
    if (set) {
      for (const handler of set) handler(params);
    }
  });
}
