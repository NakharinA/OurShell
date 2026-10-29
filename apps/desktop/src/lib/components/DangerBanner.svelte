<!--
  Persistent red banner (design `.alert`, danger colors) shown under the
  title bar on every tab - Vaults, terminal, SFTP - whenever any AI session
  currently has "dangerous mode" on. Never a silent state: there's always a
  visible kill switch right here.
-->
<script lang="ts">
  import Icon from "../icons/Icon.svelte";
  import { t, format } from "../i18n";
  import { aiSessions, anyDangerous, refreshAiStatus } from "../stores/ai";
  import { rpc } from "../rpc";
  import { toast, errorText } from "../stores/toast";

  let busy = false;

  $: dangerous = $aiSessions.filter((s) => !!s.dangerous_until);
  $: names = dangerous.map((s) => s.ai_client_name ?? s.ai_session).join(", ");
  $: hostNames = [...new Set(dangerous.flatMap((s) => s.dangerous_hosts))].join(", ");

  async function stopAll(): Promise<void> {
    busy = true;
    try {
      await rpc.aiStop();
      await refreshAiStatus();
      toast.success($t("toast.aiStopped"));
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    } finally {
      busy = false;
    }
  }
</script>

{#if $anyDangerous}
  <div class="alert danger banner-strip" role="alert">
    <Icon name="warn" />
    <span>{format($t("dm.bannerLabel"), { names })}{hostNames ? ` (${hostNames})` : ""}</span>
    <span class="spacer"></span>
    <button class="btn btn-sm" disabled={busy} on:click={stopAll}>
      <Icon name="stop" size="sm" />
      <span>{$t("ai.stopAll")}</span>
    </button>
  </div>
{/if}

<style>
  .banner-strip {
    border-radius: 0;
    flex-shrink: 0;
  }
</style>
