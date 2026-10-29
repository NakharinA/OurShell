// AI panel state: pending approvals, AI sessions, and the dangerous-mode banner.
// Kept in one store module because they all come from the same `ai.status`
// snapshot and the same handful of agent events (`approval.request`,
// `approval.resolved`, `ai.mode`).

import { derived, writable } from "svelte/store";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { rpc } from "../rpc";
import { agentStatus, onAgentEvent } from "./events";
import { EventMethod, type AiSessionInfo, type ApprovalRequest } from "../types";

export const pendingApprovals = writable<ApprovalRequest[]>([]);
export const aiSessions = writable<AiSessionInfo[]>([]);

/** True while any AI session currently has dangerous mode active. */
export const anyDangerous = derived(aiSessions, (sessions) =>
  sessions.some((s) => !!s.dangerous_until),
);

export async function refreshAiStatus(): Promise<void> {
  try {
    const status = await rpc.aiStatus();
    pendingApprovals.set(status.pending);
    aiSessions.set(status.sessions);
  } catch {
    // Vault may be locked or the agent mid-reconnect; leave the last-known
    // state alone rather than clearing a legitimate banner.
  }
}

async function notifyApproval(req: ApprovalRequest): Promise<void> {
  try {
    let granted = await isPermissionGranted();
    if (!granted) {
      granted = (await requestPermission()) === "granted";
    }
    if (granted) {
      await sendNotification({
        title: "OurShell: AI approval requested",
        body: `${req.ai_client_name ?? "An AI client"} wants to run "${req.tool}" on ${req.host}`,
      });
    }
  } catch {
    // Notifications are a convenience; never block the approval flow on them.
  }
}

/** Brings the app window to the front, e.g. when the user clicks an approval row. */
export async function focusWindow(): Promise<void> {
  try {
    const win = getCurrentWindow();
    await win.show();
    await win.setFocus();
  } catch {
    /* ignore */
  }
}

let started = false;

/** Wires AI events into the stores above. Call once after `startEventBus()`. */
export function startAiWatcher(): void {
  if (started) return;
  started = true;

  onAgentEvent(EventMethod.APPROVAL_REQUEST, (params) => {
    const req = params as ApprovalRequest;
    pendingApprovals.update((list) =>
      list.some((a) => a.approval_id === req.approval_id) ? list : [...list, req],
    );
    void notifyApproval(req);
  });

  onAgentEvent(EventMethod.APPROVAL_RESOLVED, (params) => {
    const { approval_id } = params as { approval_id: string };
    pendingApprovals.update((list) => list.filter((a) => a.approval_id !== approval_id));
  });

  onAgentEvent(EventMethod.AI_MODE, () => {
    void refreshAiStatus();
  });
  onAgentEvent(EventMethod.VAULT_UNLOCKED, () => {
    void refreshAiStatus();
  });

  // Events sent while the bridge was disconnected (agent restart, app reload)
  // are lost, so re-read the full state every time the connection comes back.
  let wasConnected = false;
  agentStatus.subscribe((s) => {
    if (s.connected && !wasConnected) void refreshAiStatus();
    wasConnected = s.connected;
  });

  void refreshAiStatus();
}
