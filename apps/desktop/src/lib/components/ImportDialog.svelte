<script lang="ts">
  import type { ImportReport } from "../types";
  function summarize(r: ImportReport): string {
    const parts = [
      `${r.hosts_added} host${r.hosts_added === 1 ? "" : "s"}`,
      `${r.keys_added} key${r.keys_added === 1 ? "" : "s"}`,
      `${r.identities_added} identit${r.identities_added === 1 ? "y" : "ies"}`,
      `${r.groups_added} group${r.groups_added === 1 ? "" : "s"}`,
    ];
    let msg = `Imported ${parts.join(", ")}.`;
    if (r.hosts_skipped.length) msg += ` Skipped existing: ${r.hosts_skipped.join(", ")}.`;
    if (r.warnings.length) msg += ` Warnings: ${r.warnings.join("; ")}`;
    return msg;
  }

  import { rpc } from "../rpc";
  import { refreshAllItems } from "../stores/items";
  import { showImport } from "../stores/ui";

  let kind: "ssh_config" | "termius_csv" = "ssh_config";
  let fileName = "";
  let contents = "";
  let baseDir = "";
  let busy = false;
  let error = "";
  let resultSummary = "";

  async function onFile(e: Event): Promise<void> {
    const input = e.target as HTMLInputElement;
    const file = input.files?.[0];
    if (!file) return;
    fileName = file.name;
    contents = await file.text();
  }

  async function run(): Promise<void> {
    if (!contents) {
      error = "Choose a file first.";
      return;
    }
    busy = true;
    error = "";
    resultSummary = "";
    try {
      const result =
        kind === "ssh_config"
          ? await rpc.importSshConfig({ contents, base_dir: baseDir || null })
          : await rpc.importTermiusCsv({ contents });
      resultSummary = summarize(result);
      await refreshAllItems();
    } catch (err) {
      error = (err as Error).message ?? "Import failed.";
    } finally {
      busy = false;
    }
  }

  function close(): void {
    showImport.set(false);
    contents = "";
    fileName = "";
    error = "";
    resultSummary = "";
  }
</script>

{#if $showImport}
  <div class="modal-backdrop" on:click={close}>
    <div class="modal" on:click|stopPropagation>
      <h2>Import</h2>
      <div class="row tabs">
        <button class:active={kind === "ssh_config"} on:click={() => (kind = "ssh_config")}>
          ~/.ssh/config
        </button>
        <button class:active={kind === "termius_csv"} on:click={() => (kind = "termius_csv")}>
          Termius CSV
        </button>
      </div>
      <label>
        File
        <input type="file" on:change={onFile} />
      </label>
      {#if fileName}<p class="text-dim">Selected: {fileName}</p>{/if}
      {#if kind === "ssh_config"}
        <label>
          Base directory (for IdentityFile / Include, optional)
          <input bind:value={baseDir} placeholder="~/.ssh" />
        </label>
      {/if}
      {#if error}<p class="error">{error}</p>{/if}
      {#if resultSummary}<p class="hint">{resultSummary}</p>{/if}
      <div class="row actions">
        <span class="spacer"></span>
        <button on:click={close}>Close</button>
        <button class="primary" on:click={run} disabled={busy || !contents}>Import</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .modal {
    width: 420px;
  }
  .tabs button.active {
    border-color: var(--accent);
    color: var(--accent);
  }
  .error {
    color: var(--danger);
    font-size: 13px;
  }
  .hint {
    color: var(--success);
    font-size: 13px;
  }
  .actions {
    margin-top: 14px;
  }
</style>
