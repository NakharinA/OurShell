// Lightweight toast notifications (design: `.toast`), e.g. "Host saved" or
// an RPC error message. Any component calls `toast.success("...")` /
// `toast.error("...")`; `<Toasts />` (mounted once) renders and auto-dismisses.

import { writable } from "svelte/store";

export interface ToastMsg {
  id: number;
  kind: "success" | "error";
  text: string;
}

export const toasts = writable<ToastMsg[]>([]);

let nextId = 1;

function push(kind: ToastMsg["kind"], text: string, ms: number): void {
  const id = nextId++;
  toasts.update((list) => [...list, { id, kind, text }]);
  setTimeout(() => dismiss(id), ms);
}

export function dismiss(id: number): void {
  toasts.update((list) => list.filter((t) => t.id !== id));
}

export const toast = {
  success(text: string, ms = 3500): void {
    push("success", text, ms);
  },
  error(text: string, ms = 5000): void {
    push("error", text, ms);
  },
};

/** Extracts a readable message from an RPC error / generic error. */
export function errorText(err: unknown, fallback: string): string {
  if (err && typeof err === "object" && "message" in err) {
    const msg = (err as { message?: unknown }).message;
    if (typeof msg === "string" && msg) return msg;
  }
  return fallback;
}
