// The list of vaults (personal + team) this account can open. Thin wrapper
// around `team.list` so every page re-fetches the same way (mirrors
// stores/items.ts).

import { derived, writable } from "svelte/store";
import { rpc } from "../rpc";
import type { TeamInfo } from "../types";

export const teams = writable<TeamInfo[]>([]);

export async function refreshTeams(): Promise<void> {
  teams.set(await rpc.teamList());
}

/** `vault_id -> display name`, for showing a human name on item rows.
 * Only team vaults are included -- a personal-vault item (the common
 * case) has no entry, so callers can treat "no entry" as "personal". */
export const vaultNames = derived(teams, ($teams) => {
  const map = new Map<string, string>();
  for (const t of $teams) {
    if (t.kind === "team") map.set(t.vault_id, t.name);
  }
  return map;
});
