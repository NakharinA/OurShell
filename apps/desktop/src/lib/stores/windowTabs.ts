// Unifies the custom titlebar's tabs (Termius style): a pinned "Vaults" home
// tab plus one tab per open session (terminal or SFTP), interleaved by the
// order they were opened. This module owns which one is currently shown
// under the titlebar; stores/terminal.ts and stores/sftp.ts own the tabs'
// actual content.

import { derived, get, writable } from "svelte/store";
import { activeTabId, closeTab, tabs } from "./terminal";
import { activeSftpTabId, closeSftpTab, sftpTabs } from "./sftp";

export type WindowTabRef =
  | { type: "vaults" }
  | { type: "terminal"; id: string }
  | { type: "sftp"; id: string };

export const activeWindowTab = writable<WindowTabRef>({ type: "vaults" });

export function focusVaultsTab(): void {
  activeWindowTab.set({ type: "vaults" });
}

export function focusTerminalTab(id: string): void {
  activeTabId.set(id);
  activeWindowTab.set({ type: "terminal", id });
}

export function focusSftpTab(id: string): void {
  activeSftpTabId.set(id);
  activeWindowTab.set({ type: "sftp", id });
}

export interface WindowTabEntry {
  type: "terminal" | "sftp";
  id: string;
  title: string;
  order: number;
}

/** Terminal + SFTP tabs, interleaved in the order they were opened. */
export const windowTabList = derived([tabs, sftpTabs], ([$tabs, $sftpTabs]) => {
  const items: WindowTabEntry[] = [
    ...$tabs.map((t) => ({ type: "terminal" as const, id: t.id, title: t.title, order: t.order })),
    ...$sftpTabs.map((t) => ({
      type: "sftp" as const,
      id: t.id,
      title: t.host.item.name,
      order: t.order,
    })),
  ];
  items.sort((a, b) => a.order - b.order);
  return items;
});

/** Closes a terminal/SFTP window tab, falling back to the Vaults tab if it was active. */
export function closeWindowTab(ref: { type: "terminal" | "sftp"; id: string }): void {
  if (ref.type === "terminal") closeTab(ref.id);
  else closeSftpTab(ref.id);

  const cur = get(activeWindowTab);
  if (cur.type !== "vaults" && cur.id === ref.id) {
    focusVaultsTab();
  }
}
