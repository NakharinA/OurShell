// Update checking for the desktop app. Two code paths, chosen automatically
// at runtime:
//
// - Release builds that carry an updater signing key (`updater` cargo
//   feature, see apps/desktop/src-tauri/Cargo.toml and
//   docs/release-signing.md) register `tauri-plugin-updater`, so
//   `@tauri-apps/plugin-updater`'s `check()` works: it fetches and verifies
//   `latest.json` (signed) from the GitHub release and does its own
//   version comparison on the Rust side.
// - Every other build (local dev, CI, or a release without a signing key)
//   has no updater plugin registered, so importing/calling it throws. We
//   fall back to the Tauri command `check_latest_release`
//   (apps/desktop/src-tauri/src/commands.rs), which asks the GitHub API for
//   the latest release from Rust -- not from this webview, so the CSP's
//   `connect-src` never needs to allow `api.github.com` -- and reports
//   whether it's newer than this build's version. That path can only offer
//   "open the download page", since it has no signed artifact to install.
//
// Both paths ultimately populate `updateInfo`; `UpdateBanner.svelte` renders
// whichever is set. `checkForUpdates(true)` (manual, from the sidebar's
// "Check for updates" item) also toasts when nothing changed; the automatic
// startup/6-hourly check stays silent unless there's something to show.

import { derived, get, writable } from "svelte/store";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { format, t } from "../i18n";
import { toast } from "./toast";

/** Mirrors `FallbackUpdateCheck` in `src-tauri/src/commands.rs`. */
interface FallbackUpdateCheck {
  current: string;
  latest: string | null;
  available: boolean;
  html_url: string | null;
  notes: string | null;
}

/** Minimal shape of `@tauri-apps/plugin-updater`'s `Update` object we use. */
interface PluginUpdate {
  version: string;
  body?: string;
  downloadAndInstall(onEvent: (event: { event: string; data?: { contentLength?: number; chunkLength?: number } }) => void): Promise<void>;
}

export type UpdateState =
  | {
      /** A signed update the plugin can download and install itself. */
      kind: "updater";
      version: string;
      notes: string;
      update: PluginUpdate;
    }
  | {
      /** No updater plugin in this build; the user installs it manually. */
      kind: "fallback";
      version: string;
      notes: string;
      htmlUrl: string;
    };

export const updateInfo = writable<UpdateState | null>(null);
export const updateInstalling = writable(false);
export const updateProgressPct = writable<number | null>(null);

/** The running app's own version (`@tauri-apps/api/app` `getVersion()`), for
 * display (sidebar "About OurShell vX.Y.Z") and for toasts. Populated once,
 * lazily, and cached -- `getVersion()` is an IPC round-trip. */
export const appVersion = writable<string>("");
let versionLoaded = false;

export async function loadAppVersion(): Promise<string> {
  if (!versionLoaded) {
    versionLoaded = true;
    try {
      appVersion.set(await getVersion());
    } catch {
      // Not running inside Tauri (e.g. a plain browser tab in dev) - leave blank.
    }
  }
  return get(appVersion);
}

/** True while the updater plugin backs the check (vs. the GitHub-API fallback). */
export const usingPluginUpdater = derived(updateInfo, ($u) => $u?.kind === "updater");

/**
 * Checks for an update. `manual` controls only the "nothing to report"
 * case: a silent no-op for the automatic startup/periodic check, a toast
 * for the sidebar's "Check for updates" item.
 */
export async function checkForUpdates(manual = false): Promise<void> {
  const current = (await loadAppVersion()) || "0.0.0";

  // Preferred path: the signed updater plugin, when this build has it.
  try {
    const mod = await import("@tauri-apps/plugin-updater");
    const update = await mod.check();
    if (update) {
      updateInfo.set({
        kind: "updater",
        version: update.version,
        notes: update.body ?? "",
        update: update as unknown as PluginUpdate,
      });
      return;
    }
    if (manual) {
      toast.success(format(get(t)("update.upToDate"), { v: current }));
    }
    return;
  } catch {
    // No updater plugin registered in this build (or a transient failure
    // reaching the endpoint) - fall through to the GitHub API fallback.
  }

  try {
    const result = await invoke<FallbackUpdateCheck>("check_latest_release");
    if (result.available && result.latest && result.html_url) {
      updateInfo.set({
        kind: "fallback",
        version: result.latest,
        notes: result.notes ?? "",
        htmlUrl: result.html_url,
      });
    } else if (manual) {
      toast.success(format(get(t)("update.upToDate"), { v: result.current || current }));
    }
  } catch {
    if (manual) {
      toast.error(get(t)("update.checkFailed"));
    }
  }
}

/** Downloads and installs an updater-plugin update, tracking progress, then relaunches. */
export async function installUpdate(): Promise<void> {
  const info = get(updateInfo);
  if (!info || info.kind !== "updater") return;

  updateInstalling.set(true);
  updateProgressPct.set(0);
  let total = 0;
  let downloaded = 0;
  try {
    await info.update.downloadAndInstall((event) => {
      if (event.event === "Started") {
        total = event.data?.contentLength ?? 0;
      } else if (event.event === "Progress") {
        downloaded += event.data?.chunkLength ?? 0;
        updateProgressPct.set(total > 0 ? Math.min(100, Math.round((downloaded / total) * 100)) : null);
      } else if (event.event === "Finished") {
        updateProgressPct.set(100);
      }
    });
    const { relaunch } = await import("@tauri-apps/plugin-process");
    await relaunch();
  } catch (err) {
    updateInstalling.set(false);
    updateProgressPct.set(null);
    toast.error(format(get(t)("update.installFailed"), { error: String(err) }));
  }
}

/** Opens the fallback path's release page in the user's browser (not the webview). */
export async function openDownloadPage(): Promise<void> {
  const info = get(updateInfo);
  if (!info || info.kind !== "fallback") return;
  try {
    const { open } = await import("@tauri-apps/plugin-shell");
    await open(info.htmlUrl);
  } catch (err) {
    toast.error(format(get(t)("update.installFailed"), { error: String(err) }));
  }
}

export function dismissUpdate(): void {
  updateInfo.set(null);
}
