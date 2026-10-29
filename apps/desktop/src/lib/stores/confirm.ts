// In-app replacement for window.confirm(): `await confirmDialog({...})`
// resolves true/false. `<ConfirmDialog />` (mounted once in App.svelte)
// renders the request in the design's modal style.

import { writable } from "svelte/store";

export interface ConfirmRequest {
  title: string;
  message?: string;
  /** Label for the confirming button; defaults to "Remove". */
  confirmLabel?: string;
  /** Red confirm button for destructive actions (default true). */
  danger?: boolean;
}

interface Pending extends ConfirmRequest {
  resolve: (ok: boolean) => void;
}

export const confirmRequest = writable<Pending | null>(null);

export function confirmDialog(req: ConfirmRequest): Promise<boolean> {
  return new Promise((resolve) => {
    // A newer request replaces an unanswered one, which counts as "cancel".
    confirmRequest.update((prev) => {
      prev?.resolve(false);
      return { danger: true, ...req, resolve };
    });
  });
}

export function answerConfirm(ok: boolean): void {
  confirmRequest.update((cur) => {
    cur?.resolve(ok);
    return null;
  });
}
