// Windows-Terminal-style clipboard keys for the SSH terminal.
//
//   Ctrl+C         copy if text is selected, otherwise send ^C (interrupt)
//   Ctrl+Shift+C   copy
//   Ctrl+V         paste
//   Ctrl+Shift+V   paste
//   Shift+Insert   paste
//   Right-click    copy the selection, or paste when nothing is selected

import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager";

export type ClipboardAction = "copy" | "paste" | null;

export interface KeyLike {
  type: string;
  key: string;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  metaKey: boolean;
}

/** What a keydown should do with the clipboard, or `null` to let the terminal handle it. */
export function clipboardAction(ev: KeyLike, hasSelection: boolean): ClipboardAction {
  if (ev.type !== "keydown" || ev.altKey || ev.metaKey) return null;
  const key = ev.key.toLowerCase();
  if (ev.shiftKey && !ev.ctrlKey && key === "insert") return "paste";
  if (!ev.ctrlKey) return null;
  if (key === "v") return "paste";
  if (key === "c" && (ev.shiftKey || hasSelection)) return "copy";
  return null;
}

/** Copies `text` with the Tauri clipboard plugin, falling back to the web API. */
export async function copyText(text: string): Promise<void> {
  try {
    await writeText(text);
  } catch {
    await navigator.clipboard.writeText(text);
  }
}

/** Reads clipboard text with the Tauri clipboard plugin, falling back to the web API. */
export async function pasteText(): Promise<string> {
  try {
    return (await readText()) ?? "";
  } catch {
    return await navigator.clipboard.readText();
  }
}
