<!--
  Update banner (design: `.update`). Checks on mount and every six hours
  (see `checkForUpdates` in stores/update.ts for the two code paths: the
  signed updater plugin, or the GitHub-API fallback when that plugin isn't
  in this build). The sidebar's "Check for updates" profile-menu item calls
  the same `checkForUpdates(true)`, which additionally toasts when there's
  nothing new.
-->
<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { t, format } from "../i18n";
  import {
    checkForUpdates,
    dismissUpdate,
    installUpdate,
    openDownloadPage,
    updateInfo,
    updateInstalling,
    updateProgressPct,
  } from "../stores/update";

  const SIX_HOURS_MS = 6 * 60 * 60 * 1000;
  let timer: ReturnType<typeof setInterval> | undefined;

  onMount(() => {
    void checkForUpdates(false);
    timer = setInterval(() => void checkForUpdates(false), SIX_HOURS_MS);
  });

  onDestroy(() => {
    if (timer) clearInterval(timer);
  });

  /** First non-empty line or two of release notes, so the banner stays compact. */
  function notesSummary(notes: string): string {
    const lines = notes
      .split("\n")
      .map((l) => l.trim())
      .filter(Boolean);
    const summary = lines.slice(0, 2).join(" ");
    return summary.length > 160 ? `${summary.slice(0, 157)}…` : summary;
  }
</script>

{#if $updateInfo}
  <div class="update" role="status">
    <span class="headline">
      <b>{format($t("update.title"), { version: $updateInfo.version })}</b>
      {#if notesSummary($updateInfo.notes)}
        <span class="notes">{notesSummary($updateInfo.notes)}</span>
      {/if}
    </span>
    {#if $updateInfo.kind === "updater"}
      <button type="button" disabled={$updateInstalling} on:click={() => installUpdate()}>
        {$updateInstalling
          ? $updateProgressPct !== null
            ? format($t("update.installingPct"), { pct: $updateProgressPct })
            : $t("update.installing")
          : $t("update.install")}
      </button>
    {:else}
      <button type="button" on:click={() => openDownloadPage()}>
        {$t("update.openDownload")}
      </button>
    {/if}
    <button type="button" class="dismiss" disabled={$updateInstalling} on:click={dismissUpdate}>
      {$t("update.later")}
    </button>
  </div>
{/if}

<style>
  .update {
    display: flex;
    gap: 0.75rem;
    align-items: center;
    padding: 0.4rem 0.75rem;
    background: var(--accent, #2563eb);
    color: #fff;
    font-size: 0.85rem;
  }
  .headline {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 0.1rem;
    min-width: 0;
  }
  .notes {
    opacity: 0.85;
    font-size: 0.8rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  button {
    background: rgba(255, 255, 255, 0.15);
    color: inherit;
    border: 1px solid rgba(255, 255, 255, 0.4);
    border-radius: 4px;
    padding: 0.2rem 0.6rem;
    cursor: pointer;
    white-space: nowrap;
  }
  button:disabled {
    opacity: 0.7;
    cursor: default;
  }
  .dismiss {
    background: transparent;
  }
</style>
