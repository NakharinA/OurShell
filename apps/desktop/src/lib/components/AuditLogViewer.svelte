<script lang="ts">
  import { onMount } from "svelte";
  import { rpc } from "../rpc";
  import type { AuditEntry } from "../types";
  import { showAuditLog } from "../stores/ui";

  let entries: AuditEntry[] = [];
  let error = "";
  let loading = false;

  async function load(): Promise<void> {
    loading = true;
    error = "";
    try {
      entries = await rpc.aiAudit({ limit: 200 });
    } catch (err) {
      error = (err as Error).message ?? "Could not load the audit log.";
    } finally {
      loading = false;
    }
  }

  function close(): void {
    showAuditLog.set(false);
  }

  onMount(load);
  $: if ($showAuditLog) void load();
</script>

<svelte:window on:keydown={(e) => $showAuditLog && e.key === "Escape" && close()} />

{#if $showAuditLog}
  <div class="modal-backdrop" role="presentation" on:click|self={close}>
    <div class="modal wide" role="dialog" aria-modal="true" aria-labelledby="audit-log-title">
      <h2 id="audit-log-title">AI audit log</h2>
      {#if error}<p class="error">{error}</p>{/if}
      {#if loading}
        <p class="text-dim">Loading…</p>
      {:else}
        <div class="scroll table-wrap">
          <table>
            <thead>
              <tr>
                <th>Time</th><th>Session</th><th>Host</th><th>Tool</th><th>Detail</th>
                <th>Mode</th><th>Approver</th><th>Outcome</th><th>Exit</th>
              </tr>
            </thead>
            <tbody>
              {#each entries as e, i (i)}
                <tr>
                  <td>{e.time}</td>
                  <td>{e.ai_session}</td>
                  <td>{e.host}</td>
                  <td>{e.tool}</td>
                  <td class="detail">{e.detail}</td>
                  <td>{e.mode}</td>
                  <td>{e.approver}</td>
                  <td>{e.outcome}</td>
                  <td>{e.exit_code ?? ""}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
      <div class="row actions">
        <span class="spacer"></span>
        <button on:click={close}>Close</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .modal.wide {
    width: 900px;
  }
  .table-wrap {
    max-height: 60vh;
  }
  .detail {
    max-width: 260px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .error {
    color: var(--danger);
    font-size: 13px;
  }
  .actions {
    margin-top: 14px;
  }
</style>
