// Account sign-in / sync status (crates/proto `account.*`). Cached here so
// the sidebar profile button and the Account & sync modal agree, without
// both re-fetching on every render.

import { writable } from "svelte/store";
import { rpc } from "../rpc";
import type { AccountStatus } from "../types";

export const accountStatus = writable<AccountStatus>({ signed_in: false });

export async function refreshAccountStatus(): Promise<void> {
  try {
    accountStatus.set(await rpc.accountStatus());
  } catch {
    // Local-only vault, or the agent doesn't support accounts yet - treat as signed out.
    accountStatus.set({ signed_in: false });
  }
}
