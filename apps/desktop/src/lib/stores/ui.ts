// Visibility / navigation state for the "Vaults" home tab: which nav view is
// active, which modals are open, and a few cross-component triggers (e.g.
// the titlebar's "+" button asking the Hosts view to focus its search box).
// Kept in one module because these are opened from more than one place
// (sidebar nav, a right-click menu, a card's header "..." menu) so they
// can't be owned by a single component.

import { writable } from "svelte/store";
import type { Group, Host, ItemRecord, Snippet } from "../types";

export type NavView = "hosts" | "keychain" | "forwarding" | "snippets" | "mcp" | "logs";

export const activeView = writable<NavView>("hosts");

function loadMini(): boolean {
  try {
    return localStorage.getItem("ourshell.sidebar-mini") === "1";
  } catch {
    return false;
  }
}
export const sidebarMini = writable<boolean>(loadMini());
sidebarMini.subscribe((v) => {
  try {
    localStorage.setItem("ourshell.sidebar-mini", v ? "1" : "0");
  } catch {
    /* ignore */
  }
});

// Hosts view: current group being browsed (null = all hosts / root), and the
// search text (shared with the quick-connect box).
export const hostsGroupFilter = writable<string | null>(null);
export const searchQuery = writable("");
/** Bumped by the titlebar's "+" to ask the Hosts view to focus & select the search box. */
export const quickConnectFocusToken = writable(0);
export function requestQuickConnectFocus(): void {
  activeView.set("hosts");
  hostsGroupFilter.set(null);
  quickConnectFocusToken.update((n) => n + 1);
}

// ----- Editors (shared between the grid's "+", a card's edit action, and
// the right-click / "..." context menus) -----

export type HostEditorState =
  | { mode: "new"; groupId?: string | null; prefill?: Partial<Host> }
  | { mode: "edit"; record: ItemRecord<Host> }
  | null;
export const hostEditor = writable<HostEditorState>(null);

export type GroupEditorState =
  | { mode: "new"; parentId?: string | null }
  | { mode: "edit"; record: ItemRecord<Group> }
  | null;
export const groupEditor = writable<GroupEditorState>(null);

export type KeyEditorState = { mode: "new"; kind?: "generate" | "import" } | null;
export const keyEditor = writable<KeyEditorState>(null);

export type SnippetEditorState =
  | { mode: "new" }
  | { mode: "edit"; record: ItemRecord<Snippet> }
  | null;
export const snippetEditor = writable<SnippetEditorState>(null);

export const forwardEditorOpen = writable(false);
/** Bumped by ForwardModal after `forward.open` succeeds, so ForwardingView re-lists. */
export const forwardsVersion = writable(0);

// AI
export const dangerousDialogSession = writable<string | null>(null);

// SFTP is a window tab now (see stores/sftp.ts); nothing to open here.

// Account & teams
export const accountModalOpen = writable(false);
export const teamCreateOpen = writable(false);
export const teamMembersOpen = writable<string | null>(null); // team vault id
export const shareHostModal = writable<ItemRecord<Host> | null>(null);

// "Run snippet..." picker: from a host's context menu (opens a new terminal
// tab for that host), or from an already-connected terminal pane's overlay
// (writes straight into that pane).
export type RunSnippetTarget =
  | { kind: "host"; host: ItemRecord<Host> }
  | { kind: "pane"; paneId: string; label: string };
export const runSnippetTarget = writable<RunSnippetTarget | null>(null);

/** Set by HostModal before opening KeyModal from its "Generate/Import key..."
 * link, so the new key's id flows back into the host form instead of just
 * refreshing the keychain. KeyModal calls this (if set) then clears it. */
export const keyPickerReturn = writable<((keyId: string) => void) | null>(null);
