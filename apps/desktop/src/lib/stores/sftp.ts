// SFTP sessions as window tabs (Termius-style: one tab per open session,
// terminal or SFTP — see stores/terminal.ts for the terminal side and
// stores/windowTabs.ts for how the two are interleaved in the titlebar).

import { writable } from "svelte/store";
import type { Host, ItemRecord } from "../types";
import { nextTabOrder } from "./tabOrder";

export interface SftpTab {
  id: string;
  host: ItemRecord<Host>;
  order: number;
}

export const sftpTabs = writable<SftpTab[]>([]);
export const activeSftpTabId = writable<string | null>(null);

/** Opens an SFTP tab for `host`, reusing an existing one for the same host. Returns its id. */
export function openSftpTab(host: ItemRecord<Host>): string {
  let id = "";
  sftpTabs.update((list) => {
    const existing = list.find((t) => t.host.id === host.id);
    if (existing) {
      id = existing.id;
      return list;
    }
    id = crypto.randomUUID();
    return [...list, { id, host, order: nextTabOrder() }];
  });
  activeSftpTabId.set(id);
  return id;
}

export function closeSftpTab(id: string): void {
  sftpTabs.update((list) => list.filter((t) => t.id !== id));
  activeSftpTabId.update((cur) => (cur === id ? null : cur));
}
