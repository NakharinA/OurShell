<script lang="ts">
  import { onMount } from "svelte";
  import { startEventBus } from "./lib/stores/events";
  import { refreshVaultStatus, vaultStatus } from "./lib/stores/vault";
  import { activeWindowTab } from "./lib/stores/windowTabs";
  import { t } from "./lib/i18n";
  import Unlock from "./lib/components/Unlock.svelte";
  import CreateVault from "./lib/components/CreateVault.svelte";
  import TitleBar from "./lib/components/TitleBar.svelte";
  import DangerBanner from "./lib/components/DangerBanner.svelte";
  import AgentStatusBanner from "./lib/components/AgentStatusBanner.svelte";
  import VaultsHome from "./lib/components/VaultsHome.svelte";
  import TerminalArea from "./lib/components/terminal/TerminalArea.svelte";
  import SftpTab from "./lib/components/SftpTab.svelte";
  import Toasts from "./lib/components/ui/Toasts.svelte";
  import ApprovalPopup from "./lib/components/modals/ApprovalPopup.svelte";
  import IconSprite from "./lib/icons/IconSprite.svelte";
  import UpdateBanner from "./lib/components/UpdateBanner.svelte";
  import { startAiWatcher } from "./lib/stores/ai";

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
      startupError = $t("unlock.waiting");
      setTimeout(() => void pollStatus(), 1000);
    }
  }

  // Approvals / dangerous-mode state must stay live on every tab (terminal,
  // SFTP, Vaults), not just while the Vaults tab happens to be mounted.
  $: if ($vaultStatus?.unlocked) startAiWatcher();
</script>

<IconSprite />

{#if !inTauri}
  <div class="loading">
    <div class="card panel" style="padding:28px;max-width:440px;text-align:center">
      <p>{$t("unlock.browserOnly")}</p>
      <p class="hint" style="margin-top:8px">{$t("unlock.browserOnlyHint")}</p>
    </div>
  </div>
{:else if !ready}
  <div class="loading">
    <p class="text-dim">{startupError || "Loading…"}</p>
  </div>
{:else if !$vaultStatus?.exists}
  <TitleBar />
  <CreateVault />
{:else if !$vaultStatus?.unlocked}
  <TitleBar />
  <Unlock />
{:else}
  <TitleBar />
  <UpdateBanner />
  <AgentStatusBanner />
  <DangerBanner />
  <div class="content-area">
    {#if $activeWindowTab.type === "vaults"}
      <VaultsHome />
    {:else if $activeWindowTab.type === "terminal"}
      <TerminalArea tabId={$activeWindowTab.id} />
    {:else}
      <SftpTab tabId={$activeWindowTab.id} />
    {/if}
  </div>
  <ApprovalPopup />
{/if}

<Toasts />

<style>
  .loading {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .content-area {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
</style>
