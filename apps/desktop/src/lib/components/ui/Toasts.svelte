<script lang="ts">
  import { dismiss, toasts } from "../../stores/toast";
  import Icon from "../../icons/Icon.svelte";
</script>

<div class="toasts">
  {#each $toasts as toast (toast.id)}
    <div class="toast {toast.kind}" role="status">
      <Icon name={toast.kind === "success" ? "checkc" : "warn"} />
      <span>{toast.text}</span>
      <button class="icon-btn sm" aria-label="Dismiss" on:click={() => dismiss(toast.id)}>
        <Icon name="x" size="sm" />
      </button>
    </div>
  {/each}
</div>
