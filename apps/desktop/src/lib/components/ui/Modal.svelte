<!--
  Generic modal shell (design: `.modal-layer` > `.modal.card.panel` with
  `.panel-head` / `.panel-body` / `.panel-foot`). Screens provide the title,
  an optional header extra (e.g. the vault chip + "..." menu) via named
  slots, the body as the default slot, and the footer buttons via the
  `footer` slot. Handles Escape-to-close, backdrop-click-to-close, and
  role="dialog" / aria-modal.
-->
<script lang="ts">
  import Icon from "../../icons/Icon.svelte";

  export let title: string;
  export let size: "md" | "wide" | "xwide" = "md";
  export let onClose: () => void;
  export let closeLabel = "Close";

  function backdropDown(e: MouseEvent): void {
    if (e.target === e.currentTarget) onClose();
  }

  function keydown(e: KeyboardEvent): void {
    if (e.key === "Escape") onClose();
  }
</script>

<svelte:window on:keydown={keydown} />

<div class="modal-layer" role="presentation" on:mousedown={backdropDown}>
  <div
    class="modal card panel"
    class:wide={size === "wide"}
    class:x-wide={size === "xwide"}
    role="dialog"
    aria-modal="true"
    aria-label={title}
  >
    <div class="panel-head">
      <div class="titles">
        <h2>{title}</h2>
        <slot name="subtitle" />
      </div>
      <slot name="headerExtra" />
      <button class="icon-btn sm" aria-label={closeLabel} title={closeLabel} on:click={onClose}>
        <Icon name="x" />
      </button>
    </div>
    <div class="panel-body">
      <slot />
    </div>
    {#if $$slots.footer}
      <div class="panel-foot">
        <slot name="footer" />
      </div>
    {/if}
  </div>
</div>
