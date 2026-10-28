<script lang="ts">
  // Shows a banner when a signed update is available. Builds without the
  // `updater` feature have no updater plugin, so the check fails quietly.
  import { onMount } from "svelte";

  let version = "";
  let installing = false;
  let error = "";
  let install: (() => Promise<void>) | null = null;

  onMount(async () => {
    try {
      const { check } = await import("@tauri-apps/plugin-updater");
      const update = await check();
      if (!update) return;
      version = update.version;
      install = async () => {
        installing = true;
        try {
          await update.downloadAndInstall();
          const { relaunch } = await import("@tauri-apps/plugin-process");
          await relaunch();
        } catch (e) {
          error = String(e);
          installing = false;
        }
      };
    } catch {
      // No updater in this build, or offline.
    }
  });
</script>

{#if version}
  <div class="update" role="status">
    <span>OurShell {version} is available.</span>
    {#if error}<span class="err">{error}</span>{/if}
    <button type="button" disabled={installing} on:click={() => install?.()}>
      {installing ? "Installing…" : "Install and restart"}
    </button>
    <button type="button" class="dismiss" on:click={() => (version = "")}>Later</button>
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
  .update span:first-child {
    flex: 1;
  }
  .err {
    color: #fee2e2;
  }
  button {
    background: rgba(255, 255, 255, 0.15);
    color: inherit;
    border: 1px solid rgba(255, 255, 255, 0.4);
    border-radius: 4px;
    padding: 0.2rem 0.6rem;
    cursor: pointer;
  }
  .dismiss {
    background: transparent;
  }
</style>
