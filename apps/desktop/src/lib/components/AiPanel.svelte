<script lang="ts">
  import { rpc } from "../rpc";
  import { aiSessions, pendingApprovals, refreshAiStatus } from "../stores/ai";
  import { showAiPanel, showAuditLog, dangerousDialogSession } from "../stores/ui";

  let busy = false;
  let error = "";

  async function decide(approvalId: string, approve: boolean): Promise<void> {
    busy = true;
    error = "";
    try {
      await rpc.aiDecide({ approval_id: approvalId, approve });
      await refreshAiStatus();
    } catch (err) {
      error = (err as Error).message ?? "Could not record the decision.";
    } finally {
      busy = false;
    }
  }

  async function turnOff(aiSession: string): Promise<void> {
    busy = true;
    try {
      await rpc.aiDangerous({ ai_session: aiSession, hosts: [], minutes: 0 });
      await refreshAiStatus();
    } finally {
      busy = false;
    }
  }

  async function stopAll(): Promise<void> {
    busy = true;
    try {
      await rpc.aiStop();
      await refreshAiStatus();
    } finally {
      busy = false;
    }
  }

  function close(): void {
    showAiPanel.set(false);
  }
</script>

{#if $showAiPanel}
  <div class="modal-backdrop" on:click={close}>
    <div class="modal wide" on:click|stopPropagation>
      <div class="row">
        <h2>AI</h2>
        <span class="spacer"></span>
        <button on:click={() => showAuditLog.set(true)}>Audit log</button>
        <button class="danger" on:click={stopAll} disabled={busy}>Stop all AI</button>
      </div>

      <h3>Pending approvals</h3>
      {#if $pendingApprovals.length === 0}
        <p class="text-dim">Nothing is waiting on you right now.</p>
      {:else}
        <table>
          <thead>
            <tr><th>Client</th><th>Host</th><th>Tool</th><th>Detail</th><th></th></tr>
          </thead>
          <tbody>
            {#each $pendingApprovals as a (a.approval_id)}
              <tr>
                <td>{a.ai_client_name ?? "AI"}</td>
                <td>{a.host}</td>
                <td>{a.tool}</td>
                <td class="detail">{a.detail}</td>
                <td class="row">
                  <button class="primary" disabled={busy} on:click={() => decide(a.approval_id, true)}>
                    Approve
                  </button>
                  <button class="danger" disabled={busy} on:click={() => decide(a.approval_id, false)}>
                    Reject
                  </button>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}

      <h3>AI sessions</h3>
      {#if $aiSessions.length === 0}
        <p class="text-dim">No AI client is connected.</p>
      {:else}
        <table>
          <thead>
            <tr><th>Client</th><th>Session</th><th>Dangerous mode</th><th></th></tr>
          </thead>
          <tbody>
            {#each $aiSessions as s (s.ai_session)}
              <tr>
                <td>{s.ai_client_name ?? "AI"}</td>
                <td class="mono">{s.ai_session}</td>
                <td>
                  {#if s.dangerous_until}
                    <span class="danger-text">on until {s.dangerous_until} ({s.dangerous_hosts.join(", ")})</span>
                  {:else}
                    off
                  {/if}
                </td>
                <td>
                  {#if s.dangerous_until}
                    <button class="danger" on:click={() => turnOff(s.ai_session)} disabled={busy}>
                      Turn off
                    </button>
                  {:else}
                    <button on:click={() => dangerousDialogSession.set(s.ai_session)}>
                      Dangerous mode…
                    </button>
                  {/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}

      {#if error}<p class="error">{error}</p>{/if}
      <div class="row actions">
        <span class="spacer"></span>
        <button on:click={close}>Close</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .modal.wide {
    width: 760px;
  }
  h3 {
    margin-top: 16px;
  }
  .detail {
    max-width: 220px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .mono {
    font-family: var(--mono);
    font-size: 12px;
  }
  .danger-text {
    color: var(--danger);
  }
  .error {
    color: var(--danger);
    font-size: 13px;
  }
  .actions {
    margin-top: 14px;
  }
</style>
