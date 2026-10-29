<script lang="ts">
  import { onMount } from "svelte";
  import { startEventBus } from "./lib/stores/events";
  import { refreshVaultStatus, vaultStatus } from "./lib/stores/vault";
  import Unlock from "./lib/components/Unlock.svelte";
  import CreateVault from "./lib/components/CreateVault.svelte";
  import MainLayout from "./lib/components/MainLayout.svelte";

  let ready = false;
  let startupError = "";
  // The UI talks to the agent through Tauri; a plain browser tab can't.
  const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

  onMount(async () => {
    if (!inTauri) return;
    await startEventBus();
    await pollStatus();
  });

  async function pollStatus(): Promise<void> {
    try {
      await refreshVaultStatus();
      ready = true;
      startupError = "";
    } catch {
      // The agent may still be starting (or spawning). Keep retrying; the
      // connecting banner inside MainLayout/Unlock screens isn't shown yet
      // because we haven't rendered them, so show a small inline message.
      startupError = "Waiting for the OurShell agent…";
      setTimeout(() => void pollStatus(), 1000);
    }
  }
</script>

{#if !inTauri}
  <div class="loading">
    <p class="text-dim">
      This page only works inside the OurShell desktop window.<br />
      Run <code>npm run tauri dev</code> in <code>apps/desktop</code> instead of opening it in a browser.
    </p>
  </div>
{:else if !ready}
  <div class="loading">
    <p class="text-dim">{startupError || "Loading…"}</p>
  </div>
{:else if !$vaultStatus?.exists}
  <CreateVault />
{:else if !$vaultStatus?.unlocked}
  <Unlock />
{:else}
  <MainLayout />
{/if}

<style>
  .loading {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
  }
</style>
