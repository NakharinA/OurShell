<script lang="ts">
  import TerminalSession from "./TerminalSession.svelte";
  import Icon from "../../icons/Icon.svelte";
  import { closePane, setActivePane, splitPane, type PaneNode } from "../../stores/terminal";
  import { hosts } from "../../stores/items";

  export let tabId: string;
  export let node: PaneNode;
  export let activePaneId: string;

  let dragIndex = -1;
  let containerEl: HTMLDivElement;

  function onDividerDown(i: number, e: MouseEvent): void {
    e.preventDefault();
    dragIndex = i;
    window.addEventListener("mousemove", onDragMove);
    window.addEventListener("mouseup", onDragUp);
  }

  function onDragMove(e: MouseEvent): void {
    if (dragIndex < 0 || node.type !== "split") return;
    const rect = containerEl.getBoundingClientRect();
    const isRow = node.orientation === "row";
    const pos = isRow ? e.clientX - rect.left : e.clientY - rect.top;
    const total = isRow ? rect.width : rect.height;
    const ratio = Math.min(0.9, Math.max(0.1, pos / total));
    // Only supports a two-way split boundary at `dragIndex`; adjusts the pair
    // of sizes on either side of the divider.
    const sizes = [...node.sizes];
    const before = sizes.slice(0, dragIndex).reduce((a, b) => a + b, 0);
    const pairTotal = sizes[dragIndex] + sizes[dragIndex + 1];
    const newBefore = Math.min(before + pairTotal - 0.05, Math.max(before + 0.05, ratio));
    sizes[dragIndex] = newBefore - before;
    sizes[dragIndex + 1] = pairTotal - sizes[dragIndex];
    node.sizes = sizes;
    node = node; // trigger reactivity
  }

  function onDragUp(): void {
    dragIndex = -1;
    window.removeEventListener("mousemove", onDragMove);
    window.removeEventListener("mouseup", onDragUp);
  }

  function doSplit(orientation: "row" | "col"): void {
    if (node.type !== "leaf") return;
    const list = $hosts;
    if (list.length === 0) return;
    // Default the new pane to the same host; the user can change it with the
    // picker in its header.
    splitPane(tabId, node.id, orientation, node.hostId, node.hostName);
  }

  function doClose(): void {
    closePane(tabId, node.id);
  }

  function onLeafKeydown(e: KeyboardEvent): void {
    // Mirrors the click handler below: Enter/Space "activates" the pane,
    // same as clicking it. The pane itself can't be a <button> because it
    // contains other interactive content (the header buttons and the
    // terminal session), so it's marked up as role="button" + tabindex
    // instead, with the matching keyboard handler a11y expects.
    // Only when the pane itself has focus: keys typed into the terminal
    // bubble up here, and cancelling them would swallow Space and Enter.
    if (e.target !== e.currentTarget) return;
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      setActivePane(tabId, node.id);
    }
  }

  function nudgeDivider(i: number, delta: number): void {
    if (node.type !== "split") return;
    const sizes = [...node.sizes];
    const before = sizes.slice(0, i).reduce((a, b) => a + b, 0);
    const pairTotal = sizes[i] + sizes[i + 1];
    const newBefore = Math.min(before + pairTotal - 0.05, Math.max(before + 0.05, before + sizes[i] + delta));
    sizes[i] = newBefore - before;
    sizes[i + 1] = pairTotal - sizes[i];
    node.sizes = sizes;
    node = node; // trigger reactivity
  }

  function onDividerKeydown(i: number, e: KeyboardEvent): void {
    if (node.type !== "split") return;
    const isRow = node.orientation === "row";
    const step = 0.02;
    const decreaseKey = isRow ? "ArrowLeft" : "ArrowUp";
    const increaseKey = isRow ? "ArrowRight" : "ArrowDown";
    if (e.key === decreaseKey) {
      e.preventDefault();
      nudgeDivider(i, -step);
    } else if (e.key === increaseKey) {
      e.preventDefault();
      nudgeDivider(i, step);
    }
  }

  // Value (0-100) the divider at index `i` currently represents, for the
  // `slider` role below.
  function dividerValue(i: number): number {
    if (node.type !== "split") return 50;
    return Math.round(node.sizes.slice(0, i + 1).reduce((a, b) => a + b, 0) * 100);
  }
</script>

{#if node.type === "leaf"}
  <div
    class="leaf"
    class:active={node.id === activePaneId}
    role="button"
    tabindex="0"
    on:click={() => setActivePane(tabId, node.id)}
    on:keydown={onLeafKeydown}
  >
    <div class="leaf-header">
      <span class="host-name">{node.hostName}</span>
      <span class="spacer"></span>
      <button class="icon-btn sm" title="Split horizontally" on:click|stopPropagation={() => doSplit("row")}>
        <Icon name="split-h" size="sm" />
      </button>
      <button class="icon-btn sm" title="Split vertically" on:click|stopPropagation={() => doSplit("col")}>
        <Icon name="split-v" size="sm" />
      </button>
      <button class="icon-btn sm" title="Close pane" on:click|stopPropagation={doClose}>
        <Icon name="x" size="sm" />
      </button>
    </div>
    <div class="leaf-body">
      <TerminalSession paneId={node.id} hostId={node.hostId} hostName={node.hostName} />
    </div>
  </div>
{:else}
  <div
    class="split"
    class:row={node.orientation === "row"}
    class:col={node.orientation === "col"}
    bind:this={containerEl}
  >
    {#each node.children as child, i (child.id)}
      <div class="split-child" style={`flex: ${node.sizes[i]} 0 0`}>
        <svelte:self {tabId} node={child} {activePaneId} />
      </div>
      {#if i < node.children.length - 1}
        <div
          class="divider"
          class:row={node.orientation === "row"}
          class:col={node.orientation === "col"}
          role="slider"
          aria-label="Resize panes"
          aria-orientation={node.orientation === "row" ? "horizontal" : "vertical"}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={dividerValue(i)}
          tabindex="0"
          on:mousedown={(e) => onDividerDown(i, e)}
          on:keydown={(e) => onDividerKeydown(i, e)}
        ></div>
      {/if}
    {/each}
  </div>
{/if}

<style>
  .leaf {
    display: flex;
    flex-direction: column;
    height: 100%;
    width: 100%;
    border: 1px solid var(--border);
    box-sizing: border-box;
  }
  .leaf.active {
    border-color: var(--accent);
  }
  .leaf-header {
    display: flex;
    align-items: center;
    gap: 2px;
    padding: 2px 6px;
    background: var(--bg-surface);
    font-size: 12px;
    border-bottom: 1px solid var(--border);
  }
  .host-name {
    color: var(--text-2);
  }
  .spacer {
    flex: 1;
  }
  .leaf-body {
    flex: 1;
    min-height: 0;
  }
  .split {
    display: flex;
    height: 100%;
    width: 100%;
  }
  .split.row {
    flex-direction: row;
  }
  .split.col {
    flex-direction: column;
  }
  .split-child {
    min-width: 0;
    min-height: 0;
    overflow: hidden;
  }
  .divider {
    background: var(--border);
    flex: 0 0 auto;
  }
  .divider.row {
    width: 4px;
    cursor: col-resize;
  }
  .divider.col {
    height: 4px;
    cursor: row-resize;
  }
</style>
