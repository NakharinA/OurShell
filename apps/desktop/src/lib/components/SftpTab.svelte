<!-- SFTP window tab (Termius-style: one tab per open session). Reuses the
     same browse/upload/download logic the old SftpBrowser modal had, styled
     like the rest of the app (breadcrumbs, table, drag & drop). -->
<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
  import { readFile, writeFile } from "@tauri-apps/plugin-fs";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import Icon from "../icons/Icon.svelte";
  import { t, format } from "../i18n";
  import { rpc, base64ToBytes, bytesToBase64 } from "../rpc";
  import type { DirEntry } from "../types";
  import { sftpTabs } from "../stores/sftp";
  import { toast, errorText } from "../stores/toast";

  export let tabId: string;

  $: tab = $sftpTabs.find((t2) => t2.id === tabId);
  $: host = tab?.host ?? null;

  let path = ".";
  let entries: DirEntry[] = [];
  let loading = false;
  let dragOver = false;
  let unlistenDrop: (() => void) | null = null;
  let lastHostId: string | null = null;

  $: if (host && host.id !== lastHostId) {
    lastHostId = host.id;
    path = ".";
    void load();
  }

  function joinPath(base: string, name: string): string {
    if (base === "." || base === "") return name;
    return base.endsWith("/") ? `${base}${name}` : `${base}/${name}`;
  }
  function parentPath(p: string): string {
    if (p === "." || p === "" || p === "/") return ".";
    const trimmed = p.endsWith("/") ? p.slice(0, -1) : p;
    const idx = trimmed.lastIndexOf("/");
    return idx <= 0 ? "." : trimmed.slice(0, idx);
  }
  $: breadcrumbs = path === "." ? [] : path.split("/").filter(Boolean);

  async function load(): Promise<void> {
    if (!host) return;
    loading = true;
    try {
      const list = await rpc.sftpList({ host: host.item.name, path });
      entries = [...list].sort((a, b) =>
        a.is_dir === b.is_dir ? a.name.localeCompare(b.name) : a.is_dir ? -1 : 1,
      );
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    } finally {
      loading = false;
    }
  }

  function open(entry: DirEntry): void {
    if (entry.is_dir) {
      path = joinPath(path, entry.name);
      void load();
    }
  }
  function up(): void {
    path = parentPath(path);
    void load();
  }
  function goTo(index: number): void {
    path = breadcrumbs.slice(0, index + 1).join("/") || ".";
    void load();
  }

  async function download(entry: DirEntry): Promise<void> {
    if (!host) return;
    const dest = await saveDialog({ defaultPath: entry.name });
    if (!dest) return;
    try {
      const file = await rpc.sftpRead({ host: host.item.name, path: joinPath(path, entry.name) });
      await writeFile(dest, base64ToBytes(file.data));
      toast.success($t("a.saved"));
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    }
  }

  async function uploadLocalPath(localPath: string): Promise<void> {
    if (!host) return;
    const name = localPath.split(/[\\/]/).pop() ?? localPath;
    try {
      const bytes = await readFile(localPath);
      await rpc.sftpWrite({ host: host.item.name, path: joinPath(path, name), data: bytesToBase64(bytes) });
      await load();
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    }
  }

  async function pickUpload(): Promise<void> {
    const selection = await openDialog({ multiple: true });
    if (!selection) return;
    const paths = Array.isArray(selection) ? selection : [selection];
    for (const p of paths) await uploadLocalPath(p);
  }

  onMount(async () => {
    try {
      unlistenDrop = await getCurrentWindow().onDragDropEvent((event) => {
        if (!host) return;
        if (event.payload.type === "over") {
          dragOver = true;
        } else if (event.payload.type === "drop") {
          dragOver = false;
          for (const p of event.payload.paths) void uploadLocalPath(p);
        } else {
          dragOver = false;
        }
      });
    } catch {
      /* drag-drop events unavailable on this platform; uploads still work via the button */
    }
  });
  onDestroy(() => unlistenDrop?.());
</script>

{#if host}
  <section class="view sftp-view" class:drag-over={dragOver}>
    <div class="card-head">
      <div class="title-row">
        <h1 class="card-title">{$t("sftp.title")} — {host.item.name}</h1>
        <span class="spacer"></span>
        <button class="btn btn-primary" on:click={pickUpload}>
          <Icon name="upload" /><span>{$t("sftp.upload")}</span>
        </button>
      </div>
      <div class="toolbar">
        <button class="btn btn-ghost btn-sm" disabled={path === "."} on:click={up}>
          <Icon name="cright" class="flip" size="sm" /><span>{$t("sftp.up")}</span>
        </button>
        <button class="link" on:click={() => { path = "."; void load(); }}>/</button>
        {#each breadcrumbs as part, i (i)}
          <Icon name="cright" size="sm" class="muted" />
          <button class="link" on:click={() => goTo(i)}>{part}</button>
        {/each}
      </div>
    </div>
    <div class="card-body">
      {#if dragOver}
        <div class="alert" style="margin-bottom:14px">
          <Icon name="upload" />
          <span>{format($t("sftp.dropHint"), { path })}</span>
        </div>
      {/if}
      {#if loading}
        <p class="hint">{$t("sftp.loading")}</p>
      {:else if entries.length === 0}
        <div class="empty"><Icon name="folder" size="lg" /><strong>{$t("sftp.empty")}</strong></div>
      {:else}
        <table class="table">
          <thead>
            <tr><th>{$t("sftp.name")}</th><th>{$t("sftp.size")}</th><th></th></tr>
          </thead>
          <tbody>
            {#each entries as entry (entry.name)}
              <tr>
                <td>
                  <button class="link" style="display:flex;align-items:center;gap:8px" on:click={() => open(entry)} disabled={!entry.is_dir}>
                    <Icon name={entry.is_dir ? "folder" : "file"} size="sm" />
                    <span style="color:var(--text)">{entry.name}</span>
                  </button>
                </td>
                <td class="hint">{entry.is_dir ? "" : entry.size ?? ""}</td>
                <td>
                  {#if !entry.is_dir}
                    <button class="icon-btn sm" title={$t("sftp.download")} on:click={() => download(entry)}>
                      <Icon name="download" size="sm" />
                    </button>
                  {/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </div>
  </section>
{/if}

<style>
  .sftp-view {
    background: var(--bg-surface);
    height: 100%;
  }
  .sftp-view.drag-over {
    box-shadow: inset 0 0 0 2px var(--accent);
  }
</style>
