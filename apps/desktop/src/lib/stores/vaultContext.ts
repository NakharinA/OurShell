// Which vault (personal or a team vault) the sidebar's vault switcher is
// currently pointed at. There's no `vault.switch` RPC - `item.list` always
// returns items from every vault the account has a key for, tagged with
// `vault_id` - so this is purely a client-side filter + "where do new items
// land" hint. Creating an item always lands it in the personal vault
// (`item.put` has no vault parameter); picking a team vault here means a
// freshly created item is immediately moved there with `team.share`.

import { derived, writable } from "svelte/store";
import type { ItemRecord, TeamInfo } from "../types";
import { teams } from "./teams";

/** `null` = personal vault. Otherwise a team vault's `vault_id`. */
export const currentVaultId = writable<string | null>(null);

export const currentVault = derived([currentVaultId, teams], ([$id, $teams]) =>
  $id ? $teams.find((t) => t.vault_id === $id) ?? null : null,
);

/** True if `record.vault_id` belongs to the vault currently selected in the sidebar. */
export function inCurrentVault<T>(record: ItemRecord<T>, vaultId: string | null): boolean {
  return vaultId ? record.vault_id === vaultId : !record.vault_id;
}

export function vaultDisplayName(vaultId: string | null, teamList: TeamInfo[]): string {
  if (!vaultId) return "Personal";
  return teamList.find((t) => t.vault_id === vaultId)?.name ?? "Team";
}
