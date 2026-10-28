<script lang="ts">
  import { rpc } from "../rpc";
  import type { ItemRecord, Snippet } from "../types";
  import { refreshSnippets, snippets } from "../stores/items";
  import { showSnippets } from "../stores/ui";
  import { activeTabId, runSnippetInPane, tabs } from "../stores/terminal";

  let editing: ItemRecord<Snippet> | null = null;
  let form: Snippet = { name: "", script: "" };
  let error = "";
  let busy = false;
  let runMessage = "";

  $: activeTab = $tabs.find((t) => t.id === $activeTabId);

  function reset(): void {
    editing = null;
    form = { name: "", script: "" };
    error = "";
  }

  function edit(rec: ItemRecord<Snippet>): void {
    editing = rec;
    form = { ...rec.item };
  }

  async function save(): Promise<void> {
    if (!form.name || !form.script) {
      error = "Name and script are required.";
      return;
    }
    busy = true;
    error = "";
    try {
      await rpc.itemPut<Snippet>({ kind: "snippet", id: editing?.id ?? null, item: form });
      await refreshSnippets();
      reset();
    } catch (err) {
      error = (err as Error).message ?? "Could not save the snippet.";
    } finally {
      busy = false;
    }
  }

  async function remove(rec: ItemRecord<Snippet>): Promise<void> {
    busy = true;
    try {
      await rpc.itemDelete({ kind: "snippet", id: rec.id });
      await refreshSnippets();
      if (editing?.id === rec.id) reset();
    } finally {
      busy = false;
    }
  }

  function run(rec: ItemRecord<Snippet>): void {
    if (!activeTab) {
      runMessage = "Open a terminal tab first.";
      return;
    }
    const ok = runSnippetInPane(activeTab.activePaneId, rec.item.script);
    runMessage = ok ? `Ran "${rec.item.name}" in ${activeTab.title}.` : "That pane isn't connected yet.";
  }

  function close(): void {
    showSnippets.set(false);
    reset();
  }
</script>

{#if $showSnippets}
  <div class="modal-backdrop" on:click={close}>
    <div class="modal wide" on:click|stopPropagation>
      <h2>Snippets</h2>
      {#if runMessage}<p class="hint">{runMessage}</p>{/if}
      <div class="layout">
        <table>
          <thead>
            <tr><th>Name</th><th></th></tr>
          </thead>
          <tbody>
            {#each $snippets as rec (rec.id)}
              <tr>
                <td>{rec.item.name}</td>
                <td class="row">
                  <button class="ghost" on:click={() => run(rec)}>Run in active terminal</button>
                  <button class="ghost" on:click={() => edit(rec)}>Edit</button>
                  <button class="ghost" on:click={() => remove(rec)}>Delete</button>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
        <form class="panel form-card" on:submit|preventDefault={save}>
          <h3>{editing ? "Edit snippet" : "New snippet"}</h3>
          <label>Name <input bind:value={form.name} /></label>
          <label>Script <textarea rows="6" bind:value={form.script}></textarea></label>
          {#if error}<p class="error">{error}</p>{/if}
          <div class="row">
            {#if editing}<button type="button" on:click={reset}>New</button>{/if}
            <span class="spacer"></span>
            <button class="primary" type="submit" disabled={busy}>Save</button>
          </div>
        </form>
      </div>
      <div class="row actions">
        <span class="spacer"></span>
        <button on:click={close}>Close</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .modal.wide {
    width: 720px;
  }
  .layout {
    display: grid;
    grid-template-columns: 1fr 260px;
    gap: 16px;
    align-items: start;
  }
  .form-card {
    padding: 14px;
    gap: 10px;
    display: flex;
    flex-direction: column;
  }
  .hint {
    color: var(--accent);
    font-size: 13px;
  }
  .error {
    color: var(--danger);
    font-size: 13px;
  }
  .actions {
    margin-top: 14px;
  }
</style>
