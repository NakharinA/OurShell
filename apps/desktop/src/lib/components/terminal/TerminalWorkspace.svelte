<script lang="ts">
  import { activeTabId, closeTab, tabs } from "../../stores/terminal";
  import PaneView from "./PaneView.svelte";

  $: activeTab = $tabs.find((t) => t.id === $activeTabId) ?? $tabs[0] ?? null;
  $: if (!$activeTabId && $tabs.length > 0) activeTabId.set($tabs[0].id);
</script>

<div class="workspace">
  {#if $tabs.length === 0}
    <div class="empty">
      <p class="text-dim">No open sessions. Pick a host from the sidebar to connect.</p>
    </div>
  {:else}
    <div class="tabbar">
      {#each $tabs as tab (tab.id)}
        <button
          class="tab"
          class:active={tab.id === activeTab?.id}
          on:click={() => activeTabId.set(tab.id)}
        >
          <span>{tab.title}</span>
          <span class="close" on:click|stopPropagation={() => closeTab(tab.id)}>✕</span>
        </button>
      {/each}
    </div>
    <div class="content">
      {#if activeTab}
        <PaneView tabId={activeTab.id} node={activeTab.root} activePaneId={activeTab.activePaneId} />
      {/if}
    </div>
  {/if}
</div>

<style>
  .workspace {
    display: flex;
    flex-direction: column;
    height: 100%;
    width: 100%;
  }
  .empty {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .tabbar {
    display: flex;
    gap: 2px;
    padding: 4px 4px 0;
    background: var(--bg-sunken);
    overflow-x: auto;
  }
  .tab {
    display: flex;
    align-items: center;
    gap: 8px;
    border-radius: 6px 6px 0 0;
    border-bottom: none;
    background: var(--bg-elevated);
  }
  .tab.active {
    background: var(--bg);
    border-color: var(--accent);
  }
  .close {
    opacity: 0.6;
  }
  .close:hover {
    opacity: 1;
  }
  .content {
    flex: 1;
    min-height: 0;
    background: var(--bg-sunken);
  }
</style>
