<script lang="ts">
  import { rpc } from "../rpc";
  import { agentStatus } from "../stores/events";
  import { aiSessions, anyDangerous, refreshAiStatus } from "../stores/ai";

  async function stopAll(): Promise<void> {
    await rpc.aiStop();
    await refreshAiStatus();
  }

  $: dangerousNames = $aiSessions
    .filter((s) => s.dangerous_until)
    .map((s) => s.ai_client_name ?? s.ai_session);
</script>

{#if $anyDangerous}
  <div class="banner danger">
    <span>⚠ Dangerous mode is ON for: {dangerousNames.join(", ")} — AI can act without approval.</span>
    <button class="danger" on:click={stopAll}>Stop all AI</button>
  </div>
{/if}
{#if !$agentStatus.connected}
  <div class="banner info">
    <span>Not connected to the agent ({$agentStatus.detail})…</span>
  </div>
{/if}

<style>
  .banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 6px 14px;
    font-size: 13px;
  }
  .banner.danger {
    background: var(--danger);
    color: white;
  }
  .banner.danger button {
    background: white;
    color: var(--danger);
    border-color: white;
  }
  .banner.info {
    background: var(--warning);
    color: var(--accent-contrast);
  }
</style>
