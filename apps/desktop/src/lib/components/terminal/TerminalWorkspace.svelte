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
        <div class="tab" class:active={tab.id === activeTab?.id}>
          <button type="button" class="tab-select" on:click={() => activeTabId.set(tab.id)}>
            <span>{tab.title}</span>
          </button>
          <button
            type="button"
            class="close"
            on:click|stopPropagation={() => closeTab(tab.id)}
            aria-label={`Close ${tab.title}`}
          >
            ✕
          </button>
        </div>
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
  /* `.tab` used to be the clickable element (a <button>), styled by the
     global `button` rule in app.css plus the overrides below. Now that it
     wraps two real buttons (select + close, so the close control isn't a
     button nested inside another button), it carries those same visual
     styles itself; the inner buttons are reset to look like plain inline
     content. */
  .tab {
    display: flex;
    align-items: center;
    gap: 8px;
    font: inherit;
    color: var(--text);
    border: 1px solid var(--panel-border);
    border-radius: 6px 6px 0 0;
    border-bottom: none;
    background: var(--bg-elevated);
    padding: 6px 12px;
    cursor: pointer;
  }
  .tab:hover {
    background: var(--hover-bg);
  }
  .tab.active {
    background: var(--bg);
    border-color: var(--accent);
  }
  .tab-select {
    font: inherit;
    color: inherit;
    background: transparent;
    border: none;
    padding: 0;
    cursor: pointer;
  }
  .close {
    font: inherit;
    color: inherit;
    background: transparent;
    border: none;
    padding: 0;
    cursor: pointer;
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
