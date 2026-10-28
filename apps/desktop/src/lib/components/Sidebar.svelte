<script lang="ts">
  import { rpc } from "../rpc";
  import { buildGroupTree, groups, hosts } from "../stores/items";
  import {
    hostEditor,
    showAiPanel,
    showForwards,
    showIdentities,
    showImport,
    showKeys,
    showSnippets,
    searchQuery,
  } from "../stores/ui";
  import { pendingApprovals } from "../stores/ai";
  import { refreshVaultStatus } from "../stores/vault";
  import GroupNodeView from "./GroupNodeView.svelte";
  import ThemePicker from "./ThemePicker.svelte";

  $: tree = buildGroupTree($groups, $hosts);

  async function lock(): Promise<void> {
    await rpc.vaultLock();
    await refreshVaultStatus();
  }
</script>

<aside class="sidebar">
  <div class="search-row">
    <input placeholder="Search hosts…" bind:value={$searchQuery} />
    <button class="primary" title="New host" on:click={() => hostEditor.set({ mode: "new" })}>+</button>
  </div>

  <nav class="tree scroll">
    <GroupNodeView node={tree} filter={$searchQuery} />
    {#if $hosts.length === 0}
      <p class="text-dim empty-hint">No hosts yet. Click "+" to add one, or use Import below.</p>
    {/if}
  </nav>

  <div class="actions col">
    <button on:click={() => showIdentities.set(true)}>Identities</button>
    <button on:click={() => showKeys.set(true)}>Keys</button>
    <button on:click={() => showSnippets.set(true)}>Snippets</button>
    <button on:click={() => showForwards.set(true)}>Port forwards</button>
    <button on:click={() => showImport.set(true)}>Import…</button>
    <button on:click={() => showAiPanel.set(true)}>
      AI{#if $pendingApprovals.length > 0}<span class="badge">{$pendingApprovals.length}</span>{/if}
    </button>
    <ThemePicker />
    <button class="danger" on:click={lock}>Lock vault</button>
  </div>
</aside>

<style>
  .sidebar {
    width: 260px;
    min-width: 200px;
    max-width: 400px;
    display: flex;
    flex-direction: column;
    border-right: 1px solid var(--panel-border);
    background: var(--bg-elevated);
  }
  .search-row {
    display: flex;
    gap: 6px;
    padding: 8px;
  }
  .search-row input {
    flex: 1;
  }
  .tree {
    flex: 1;
    padding: 4px 8px;
  }
  .empty-hint {
    padding: 8px;
    font-size: 12px;
  }
  .actions {
    padding: 8px;
    border-top: 1px solid var(--panel-border);
  }
  .badge {
    background: var(--danger);
    color: white;
    border-radius: 10px;
    padding: 0 6px;
    font-size: 11px;
    margin-left: 6px;
  }
</style>
