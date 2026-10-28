// UI-only state for the terminal workspace: tabs, each holding a tree of split
// panes. Not part of the wire protocol - just how the desktop app arranges
// however many `pty.open` sessions the user has open at once.

import { writable } from "svelte/store";

export type Orientation = "row" | "col";

export interface LeafPane {
  type: "leaf";
  id: string;
  hostId: string;
  hostName: string;
}

export interface SplitPane {
  type: "split";
  id: string;
  orientation: Orientation;
  children: PaneNode[];
  /** Fractional sizes (sum to 1) matching `children`. */
  sizes: number[];
}

export type PaneNode = LeafPane | SplitPane;

export interface TermTab {
  id: string;
  title: string;
  root: PaneNode;
  activePaneId: string;
}

export const tabs = writable<TermTab[]>([]);
export const activeTabId = writable<string | null>(null);

function genId(): string {
  return crypto.randomUUID();
}

function makeLeaf(hostId: string, hostName: string): LeafPane {
  return { type: "leaf", id: genId(), hostId, hostName };
}

/** Opens a brand-new tab with a single pane connected to `hostId`. */
export function openHostTab(hostId: string, hostName: string): void {
  const leaf = makeLeaf(hostId, hostName);
  const tab: TermTab = {
    id: genId(),
    title: hostName,
    root: leaf,
    activePaneId: leaf.id,
  };
  tabs.update((list) => [...list, tab]);
  activeTabId.set(tab.id);
}

function findNode(root: PaneNode, id: string): PaneNode | null {
  if (root.id === id) return root;
  if (root.type === "split") {
    for (const child of root.children) {
      const found = findNode(child, id);
      if (found) return found;
    }
  }
  return null;
}

function replaceNode(root: PaneNode, id: string, replacement: PaneNode): PaneNode {
  if (root.id === id) return replacement;
  if (root.type === "split") {
    return {
      ...root,
      children: root.children.map((c) => replaceNode(c, id, replacement)),
    };
  }
  return root;
}

/** Splits the pane `paneId` in tab `tabId`, keeping its session and adding a new
 * pane connected to `hostId` alongside it. */
export function splitPane(
  tabId: string,
  paneId: string,
  orientation: Orientation,
  hostId: string,
  hostName: string,
): void {
  tabs.update((list) =>
    list.map((tab) => {
      if (tab.id !== tabId) return tab;
      const existing = findNode(tab.root, paneId);
      if (!existing) return tab;
      const newLeaf = makeLeaf(hostId, hostName);
      const split: SplitPane = {
        type: "split",
        id: genId(),
        orientation,
        children: [existing, newLeaf],
        sizes: [0.5, 0.5],
      };
      return {
        ...tab,
        root: replaceNode(tab.root, paneId, split),
        activePaneId: newLeaf.id,
      };
    }),
  );
}

/** Removes one pane. Collapses a split left with a single child, and closes the
 * whole tab if its last pane is removed. */
export function closePane(tabId: string, paneId: string): void {
  tabs.update((list) => {
    const out: TermTab[] = [];
    for (const tab of list) {
      if (tab.id !== tabId) {
        out.push(tab);
        continue;
      }
      if (tab.root.id === paneId) {
        // Closing the only pane closes the tab.
        continue;
      }
      const newRoot = removeFromTree(tab.root, paneId);
      const nextActive = newRoot ? firstLeafId(newRoot) : paneId;
      out.push({ ...tab, root: newRoot ?? tab.root, activePaneId: nextActive });
    }
    return out;
  });
}

function removeFromTree(node: PaneNode, id: string): PaneNode | null {
  if (node.type !== "split") return node;
  const children = node.children
    .map((c) => (c.id === id ? null : c.type === "split" ? removeFromTree(c, id) : c))
    .filter((c): c is PaneNode => c !== null);
  if (children.length === 0) return null;
  if (children.length === 1) return children[0];
  const evenSize = 1 / children.length;
  return { ...node, children, sizes: children.map(() => evenSize) };
}

function firstLeafId(node: PaneNode): string {
  return node.type === "leaf" ? node.id : firstLeafId(node.children[0]);
}

export function closeTab(tabId: string): void {
  tabs.update((list) => list.filter((t) => t.id !== tabId));
  activeTabId.update((cur) => (cur === tabId ? null : cur));
}

export function setActivePane(tabId: string, paneId: string): void {
  tabs.update((list) =>
    list.map((t) => (t.id === tabId ? { ...t, activePaneId: paneId } : t)),
  );
}

// ----- Snippet runner: lets the snippets panel write into whichever pane is
// currently active, without every component needing a prop-drilled callback. -----

const paneWriters = new Map<string, (text: string) => void>();

export function registerPaneWriter(paneId: string, writer: (text: string) => void): void {
  paneWriters.set(paneId, writer);
}

export function unregisterPaneWriter(paneId: string): void {
  paneWriters.delete(paneId);
}

export function runSnippetInPane(paneId: string, script: string): boolean {
  const writer = paneWriters.get(paneId);
  if (!writer) return false;
  const text = script.endsWith("\n") ? script : `${script}\n`;
  writer(text);
  return true;
}
