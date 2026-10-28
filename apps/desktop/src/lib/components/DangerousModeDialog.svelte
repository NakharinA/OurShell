<script lang="ts">
  import { rpc } from "../rpc";
  import { hosts } from "../stores/items";
  import { dangerousDialogSession } from "../stores/ui";
  import { refreshAiStatus } from "../stores/ai";

  $: sessionId = $dangerousDialogSession;
  $: aiAllowedHosts = $hosts.filter((h) => h.item.ai_allowed);

  let selected = new Set<string>();
  let minutes = 60;
  let error = "";
  let busy = false;
  let understood = false;

  $: if (sessionId) {
    selected = new Set();
    minutes = 60;
    error = "";
    understood = false;
  }

  function toggle(name: string): void {
    if (selected.has(name)) selected.delete(name);
    else selected.add(name);
    selected = new Set(selected);
  }

  async function confirm(): Promise<void> {
    if (!sessionId || selected.size === 0) {
      error = "Choose at least one AI-allowed host.";
      return;
    }
    busy = true;
    error = "";
    try {
      await rpc.aiDangerous({
        ai_session: sessionId,
        hosts: [...selected],
        minutes,
      });
      await refreshAiStatus();
      dangerousDialogSession.set(null);
    } catch (err) {
      error = (err as Error).message ?? "Could not turn on dangerous mode.";
    } finally {
      busy = false;
    }
  }

  function cancel(): void {
    dangerousDialogSession.set(null);
  }
</script>

{#if sessionId}
  <div class="modal-backdrop" on:click={cancel}>
    <div class="modal danger-modal" on:click|stopPropagation>
      <h2 class="danger-title">⚠ Turn on dangerous mode</h2>
      <p>
        While dangerous mode is on, this AI session can run commands and read/write files on the
        hosts you pick below <strong>without asking you first</strong>. A red banner will stay on
        screen the whole time. This never widens which hosts are reachable - only hosts already
        marked "AI allowed" can be picked.
      </p>
      <p class="danger-text">
        If the AI is tricked by something it reads (prompt injection), it could run a harmful
        command on these hosts with nobody approving it in the moment.
      </p>
      <div class="col hosts">
        <strong>AI-allowed hosts</strong>
        {#if aiAllowedHosts.length === 0}
          <p class="text-dim">No hosts are marked "AI allowed" yet.</p>
        {/if}
        {#each aiAllowedHosts as h (h.id)}
          <label class="checkbox">
            <input
              type="checkbox"
              checked={selected.has(h.item.name)}
              on:change={() => toggle(h.item.name)}
            />
            {h.item.name}
          </label>
        {/each}
      </div>
      <label>
        Duration (minutes)
        <input type="number" min="1" bind:value={minutes} />
      </label>
      <label class="checkbox">
        <input type="checkbox" bind:checked={understood} />
        I understand the risk and want to proceed.
      </label>
      {#if error}<p class="error">{error}</p>{/if}
      <div class="row actions">
        <span class="spacer"></span>
        <button on:click={cancel} disabled={busy}>Cancel</button>
        <button class="danger" on:click={confirm} disabled={busy || !understood}>
          Turn on dangerous mode
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .danger-modal {
    width: 460px;
    border-color: var(--danger);
  }
  .danger-title {
    color: var(--danger);
  }
  .danger-text {
    color: var(--danger);
    font-weight: 500;
  }
  .hosts {
    background: var(--bg-sunken);
    border-radius: var(--radius);
    padding: 10px;
    max-height: 160px;
    overflow: auto;
  }
  .checkbox {
    flex-direction: row;
    align-items: center;
    gap: 8px;
  }
  .error {
    color: var(--danger);
    font-size: 13px;
  }
  .actions {
    margin-top: 10px;
  }
</style>
