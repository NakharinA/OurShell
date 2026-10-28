<script lang="ts">
  import type { GroupNode } from "../stores/items";
  import { openHostTab } from "../stores/terminal";
  import { hostEditor, sftpHost } from "../stores/ui";

  export let node: GroupNode;
  export let depth = 0;
  export let filter = "";

  function matches(name: string): boolean {
    return !filter || name.toLowerCase().includes(filter.toLowerCase());
  }

  $: visibleHosts = node.hosts.filter((h) => matches(h.item.name));
</script>

<div class="indent" style={`--depth: ${depth}`}>
  {#if node.group}
    <div class="group-row">
      <strong>{node.group.item.name}</strong>
    </div>
  {/if}
  {#each visibleHosts as h (h.id)}
    <div class="host-row">
      <button class="host-btn" on:click={() => openHostTab(h.id, h.item.name)}>
        <span class="dot" class:ai={h.item.ai_allowed}></span>
        {h.item.name}
      </button>
      <button class="icon-btn" title="Edit" on:click={() => hostEditor.set({ mode: "edit", record: h })}>
        ✎
      </button>
      <button class="icon-btn" title="SFTP" on:click={() => sftpHost.set(h)}>⇄</button>
    </div>
  {/each}
  {#each node.children as child (child.group?.id)}
    <svelte:self node={child} depth={depth + 1} {filter} />
  {/each}
</div>

<style>
  .indent {
    padding-left: calc(var(--depth) * 12px);
  }
  .group-row {
    padding: 4px 2px;
    color: var(--text-dim);
    font-size: 12px;
    text-transform: uppercase;
  }
  .host-row {
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .host-btn {
    flex: 1;
    text-align: left;
    background: transparent;
    border: none;
    padding: 4px 6px;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .host-btn:hover {
    background: var(--hover-bg);
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--panel-border);
  }
  .dot.ai {
    background: var(--success);
  }
  .icon-btn {
    background: transparent;
    border: none;
    padding: 4px 6px;
    opacity: 0.6;
  }
  .icon-btn:hover {
    opacity: 1;
    background: var(--hover-bg);
  }
</style>
