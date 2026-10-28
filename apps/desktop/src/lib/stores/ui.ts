// Visibility state for modals/panels that can be opened from more than one
// place (e.g. the sidebar and a right-click menu), so they're not owned by any
// single component.

import { writable } from "svelte/store";
import type { Group, Host, ItemRecord } from "../types";

export type HostEditorState = { mode: "new"; groupId?: string | null } | { mode: "edit"; record: ItemRecord<Host> } | null;

export const hostEditor = writable<HostEditorState>(null);
export const groupEditor = writable<{ mode: "new"; parentId?: string | null } | { mode: "edit"; record: ItemRecord<Group> } | null>(null);
export const showIdentities = writable(false);
export const showKeys = writable(false);
export const showSnippets = writable(false);
export const showImport = writable(false);
export const showForwards = writable(false);
export const showAiPanel = writable(false);
export const showAuditLog = writable(false);
export const showTeams = writable(false);
export const sftpHost = writable<ItemRecord<Host> | null>(null);
export const dangerousDialogSession = writable<string | null>(null);
export const searchQuery = writable("");
