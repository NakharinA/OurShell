// Vault item lists (hosts, identities, keys, snippets, groups). Thin wrappers
// around `item.*` so every page re-fetches the same way.

import { writable } from "svelte/store";
import { rpc } from "../rpc";
import type { Group, Host, Identity, ItemRecord, Snippet, SshKey } from "../types";

export const hosts = writable<ItemRecord<Host>[]>([]);
export const identities = writable<ItemRecord<Identity>[]>([]);
export const keys = writable<ItemRecord<SshKey>[]>([]);
export const snippets = writable<ItemRecord<Snippet>[]>([]);
export const groups = writable<ItemRecord<Group>[]>([]);

export async function refreshHosts(): Promise<void> {
  hosts.set(await rpc.itemList<Host>("host"));
}
export async function refreshIdentities(): Promise<void> {
  identities.set(await rpc.itemList<Identity>("identity"));
}
export async function refreshKeys(): Promise<void> {
  keys.set(await rpc.itemList<SshKey>("key"));
}
export async function refreshSnippets(): Promise<void> {
  snippets.set(await rpc.itemList<Snippet>("snippet"));
}
export async function refreshGroups(): Promise<void> {
  groups.set(await rpc.itemList<Group>("group"));
}

export async function refreshAllItems(): Promise<void> {
  await Promise.all([
    refreshHosts(),
    refreshIdentities(),
    refreshKeys(),
    refreshSnippets(),
    refreshGroups(),
  ]);
}

export interface GroupNode {
  group: ItemRecord<Group> | null; // null = the synthetic root
  children: GroupNode[];
  hosts: ItemRecord<Host>[];
}

/** Builds a group tree (by `parent_id`) with each group's direct hosts attached. */
export function buildGroupTree(
  groupList: ItemRecord<Group>[],
  hostList: ItemRecord<Host>[],
): GroupNode {
  const nodeById = new Map<string, GroupNode>();
  for (const g of groupList) {
    nodeById.set(g.id, { group: g, children: [], hosts: [] });
  }
  const root: GroupNode = { group: null, children: [], hosts: [] };
  for (const g of groupList) {
    const node = nodeById.get(g.id)!;
    const parentId = g.item.parent_id;
    const parent = (parentId && nodeById.get(parentId)) || root;
    parent.children.push(node);
  }
  for (const h of hostList) {
    const groupId = h.item.group_id;
    const parent = (groupId && nodeById.get(groupId)) || root;
    parent.hosts.push(h);
  }
  return root;
}
