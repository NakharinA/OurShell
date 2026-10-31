// Host-group tree helpers. Groups form a tree through `parent_id`; these work
// on the flat list the agent returns and tolerate dangling parents or cycles
// in data written by older clients.

import type { Group, Host, ItemRecord } from "./types";

/** Ids of every group below `id` (not including `id` itself). */
export function descendantIds(groups: ItemRecord<Group>[], id: string): Set<string> {
  const out = new Set<string>();
  const queue = [id];
  while (queue.length) {
    const cur = queue.shift()!;
    for (const g of groups) {
      if (g.item.parent_id === cur && g.id !== id && !out.has(g.id)) {
        out.add(g.id);
        queue.push(g.id);
      }
    }
  }
  return out;
}

/** Groups that may become `id`'s parent: anything but itself and its descendants. */
export function parentCandidates(groups: ItemRecord<Group>[], id: string | null): ItemRecord<Group>[] {
  if (!id) return groups;
  const below = descendantIds(groups, id);
  return groups.filter((g) => g.id !== id && !below.has(g.id));
}

/** Root-first chain of groups ending at `id`. */
export function groupPath(groups: ItemRecord<Group>[], id: string | null): ItemRecord<Group>[] {
  const out: ItemRecord<Group>[] = [];
  const seen = new Set<string>();
  let cur = id;
  while (cur && !seen.has(cur)) {
    seen.add(cur);
    const g = groups.find((x) => x.id === cur);
    if (!g) break;
    out.unshift(g);
    cur = g.item.parent_id;
  }
  return out;
}

/** Hosts in group `id` or any group below it. */
export function hostCount(groups: ItemRecord<Group>[], hosts: ItemRecord<Host>[], id: string): number {
  const ids = descendantIds(groups, id);
  ids.add(id);
  return hosts.filter((h) => h.item.group_id !== null && ids.has(h.item.group_id)).length;
}
