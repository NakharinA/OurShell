<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
  import { readFile, writeFile } from "@tauri-apps/plugin-fs";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { rpc, base64ToBytes, bytesToBase64 } from "../rpc";
  import type { DirEntry } from "../types";
  import { sftpHost } from "../stores/ui";

  let path = ".";
  let entries: DirEntry[] = [];
  let loading = false;
  let error = "";
  let dragOver = false;
  let unlistenDrop: (() => void) | null = null;

  $: host = $sftpHost;
  $: if (host) {
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

  async function load(): Promise<void> {
    if (!host) return;
    loading = true;
    error = "";
    try {
      const list = await rpc.sftpList({ host: host.item.name, path });
      entries = [...list].sort((a, b) =>
        a.is_dir === b.is_dir ? a.name.localeCompare(b.name) : a.is_dir ? -1 : 1,
      );
    } catch (err) {
      error = (err as Error).message ?? "Could not list that directory.";
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

  async function download(entry: DirEntry): Promise<void> {
    if (!host) return;
    const dest = await saveDialog({ defaultPath: entry.name });
    if (!dest) return;
    error = "";
    try {
      const file = await rpc.sftpRead({ host: host.item.name, path: joinPath(path, entry.name) });
      await writeFile(dest, base64ToBytes(file.data));
    } catch (err) {
      error = (err as Error).message ?? "Download failed.";
    }
  }

  async function uploadLocalPath(localPath: string): Promise<void> {
    if (!host) return;
    const name = localPath.split(/[\\/]/).pop() ?? localPath;
    try {
      const bytes = await readFile(localPath);
      await rpc.sftpWrite({
        host: host.item.name,
        path: joinPath(path, name),
        data: bytesToBase64(bytes),
      });
      await load();
    } catch (err) {
      error = (err as Error).message ?? `Could not upload ${name}.`;
    }
  }

  async function pickUpload(): Promise<void> {
    const selection = await openDialog({ multiple: true });
    if (!selection) return;
    const paths = Array.isArray(selection) ? selection : [selection];
    for (const p of paths) await uploadLocalPath(p);
  }

  function close(): void {
    sftpHost.set(null);
  }

  onMount(async () => {
    try {
      unlistenDrop = await getCurrentWindow().onDragDropEvent((event) => {
        if (!$sftpHost) return;
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

  onDestroy(() => {
    unlistenDrop?.();
  });
</script>

<svelte:window on:keydown={(e) => host && e.key === "Escape" && close()} />

{#if host}
  <div class="modal-backdrop" role="presentation" on:click|self={close}>
    <div
      class="modal wide"
      class:drag-over={dragOver}
      role="dialog"
      aria-modal="true"
      aria-labelledby="sftp-title"
    >
      <h2 id="sftp-title">SFTP — {host.item.name}</h2>
      <div class="row path-row">
        <button class="ghost" on:click={up} disabled={path === "."}>↑ Up</button>
        <code class="path">{path}</code>
        <span class="spacer"></span>
        <button on:click={pickUpload}>Upload…</button>
      </div>
      {#if dragOver}
        <p class="hint">Drop to upload into {path}</p>
      {/if}
      {#if error}<p class="error">{error}</p>{/if}
      {#if loading}
        <p class="text-dim">Loading…</p>
      {:else}
        <table>
          <thead>
            <tr><th>Name</th><th>Size</th><th></th></tr>
          </thead>
          <tbody>
            {#each entries as entry (entry.name)}
              <tr>
                <td class="name" class:dir={entry.is_dir} on:click={() => open(entry)}>
                  {entry.is_dir ? "📁" : "📄"} {entry.name}
                </td>
                <td>{entry.is_dir ? "" : (entry.size ?? "")}</td>
                <td>
                  {#if !entry.is_dir}
                    <button class="ghost" on:click={() => download(entry)}>Download</button>
                  {/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
      <div class="row actions">
        <span class="spacer"></span>
        <button on:click={close}>Close</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .modal.wide {
    width: 640px;
  }
  .modal.drag-over {
    outline: 2px dashed var(--accent);
  }
  .path-row {
    margin-bottom: 8px;
  }
  .path {
    background: var(--bg-sunken);
    padding: 4px 8px;
    border-radius: 4px;
  }
  .name.dir {
    cursor: pointer;
    color: var(--accent);
  }
  .error {
    color: var(--danger);
    font-size: 13px;
  }
  .hint {
    color: var(--accent);
    font-size: 13px;
  }
  .actions {
    margin-top: 14px;
  }
</style>
