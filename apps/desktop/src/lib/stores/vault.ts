import { writable } from "svelte/store";
import { rpc } from "../rpc";
import type { VaultStatus } from "../types";
import { vaultLockedTick } from "./events";

export const vaultStatus = writable<VaultStatus | null>(null);

export async function refreshVaultStatus(): Promise<VaultStatus> {
  const status = await rpc.vaultStatus();
  vaultStatus.set(status);
  return status;
}

// Whenever the agent tells us the vault locked (idle timeout, explicit lock from
// another front-end, etc.), re-fetch status so the UI falls back to the unlock
// screen.
vaultLockedTick.subscribe((n) => {
  if (n > 0) {
    refreshVaultStatus().catch(() => {
      /* agent may be mid-reconnect; the next status poll will retry */
    });
  }
});
