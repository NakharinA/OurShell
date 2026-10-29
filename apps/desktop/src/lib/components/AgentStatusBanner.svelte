<!-- Thin strip shown whenever the Tauri app has lost its connection to the
     local `ourshell agent` process (reconnecting, agent crashed, etc). -->
<script lang="ts">
  import { agentStatus } from "../stores/events";
</script>

{#if !$agentStatus.connected}
  <div class="alert banner-strip" role="status">
    <span>Not connected to the agent ({$agentStatus.detail})…</span>
  </div>
{/if}

<style>
  .banner-strip {
    border-radius: 0;
    flex-shrink: 0;
  }
</style>
