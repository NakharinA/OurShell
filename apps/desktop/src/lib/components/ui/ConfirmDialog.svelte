<!--
  Styled confirmation dialog (replaces window.confirm). Sits above any open
  modal; Escape cancels it without also closing the modal underneath.
-->
<script lang="ts">
  import { tick } from "svelte";
  import Icon from "../../icons/Icon.svelte";
  import { t } from "../../i18n";
  import { confirmRequest, answerConfirm } from "../../stores/confirm";

  let cancelBtn: HTMLButtonElement | undefined;

  // Focus "Cancel" so a stray Enter never confirms a destructive action.
  $: if ($confirmRequest) void tick().then(() => cancelBtn?.focus());

  function keydown(e: KeyboardEvent): void {
    if (!$confirmRequest) return;
    if (e.key === "Escape") {
      e.stopImmediatePropagation();
      e.preventDefault();
      answerConfirm(false);
    }
  }

  function backdropDown(e: MouseEvent): void {
    if (e.target === e.currentTarget) answerConfirm(false);
  }
</script>

<svelte:window on:keydown|capture={keydown} />

{#if $confirmRequest}
  <div class="modal-layer confirm-layer" role="presentation" on:mousedown={backdropDown}>
    <div
      class="modal card panel confirm"
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="confirm-title"
      aria-describedby={$confirmRequest.message ? "confirm-msg" : undefined}
    >
      <div class="confirm-body">
        <span class="round" class:danger={$confirmRequest.danger}>
          <Icon name={$confirmRequest.danger ? "trash" : "warn"} />
        </span>
        <div class="confirm-text">
          <h2 id="confirm-title">{$confirmRequest.title}</h2>
          {#if $confirmRequest.message}
            <p id="confirm-msg" class="hint">{$confirmRequest.message}</p>
          {/if}
        </div>
      </div>
      <div class="panel-foot">
        <button class="btn btn-ghost" bind:this={cancelBtn} on:click={() => answerConfirm(false)}>
          {$t("a.cancel")}
        </button>
        <button
          class="btn"
          class:btn-danger={$confirmRequest.danger}
          class:btn-primary={!$confirmRequest.danger}
          on:click={() => answerConfirm(true)}
        >
          {$confirmRequest.confirmLabel ?? $t("a.remove")}
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .confirm-layer {
    z-index: 95;
  }
  .confirm {
    width: min(440px, 100%);
  }
  .confirm-body {
    display: flex;
    gap: 16px;
    align-items: flex-start;
    padding: 24px 28px 20px;
  }
  .confirm-text {
    min-width: 0;
    flex: 1;
  }
  .confirm-text h2 {
    font-size: 17px;
    font-weight: 600;
    overflow-wrap: anywhere;
  }
  .confirm-text .hint {
    margin-top: 6px;
    font-size: 13px;
  }
  .round.danger {
    background: var(--danger-soft);
    color: var(--danger-text);
  }
  .btn-danger {
    background: var(--danger);
    color: #fff;
  }
  .btn-danger:hover {
    background: var(--danger-text);
    box-shadow: 0 4px 12px var(--danger-soft);
  }
</style>
