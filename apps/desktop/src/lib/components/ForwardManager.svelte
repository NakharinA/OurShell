<script lang="ts">
  import { onMount } from "svelte";
  import { rpc } from "../rpc";
  import type { ForwardInfo, ForwardKind } from "../types";
  import { hosts } from "../stores/items";
  import { showForwards } from "../stores/ui";

  let forwards: ForwardInfo[] = [];
  let hostName = "";
  let kind: ForwardKind = "local";
  let bind = "127.0.0.1:8080";
  let dest = "";
  let error = "";
  let busy = false;

  async function refresh(): Promise<void> {
    try {
      forwards = await rpc.forwardList();
    } catch (err) {
      error = (err as Error).message ?? "Could not list forwards.";
    }
  }

  async function openForward(): Promise<void> {
    if (!hostName || !bind || !dest) {
      error = "Host, bind address and destination are all required.";
      return;
    }
    busy = true;
    error = "";
    try {
      await rpc.forwardOpen({ host: hostName, kind, bind, dest });
      await refresh();
    } catch (err) {
      error = (err as Error).message ?? "Could not open the forward.";
    } finally {
      busy = false;
    }
  }

  async function closeForward(id: string): Promise<void> {
    busy = true;
    try {
      await rpc.forwardClose(id);
      await refresh();
    } finally {
      busy = false;
    }
  }

  function close(): void {
    showForwards.set(false);
  }

  onMount(refresh);
  $: if ($showForwards) void refresh();
</script>

{#if $showForwards}
  <div class="modal-backdrop" on:click={close}>
    <div class="modal wide" on:click|stopPropagation>
      <h2>Port forwards</h2>
      <table>
        <thead>
          <tr><th>Host</th><th>Kind</th><th>Bind</th><th>Destination</th><th></th></tr>
        </thead>
        <tbody>
          {#each forwards as f (f.id)}
            <tr>
              <td>{f.host}</td>
              <td>{f.kind}</td>
              <td>{f.bind}</td>
              <td>{f.dest}</td>
              <td><button class="ghost" on:click={() => closeForward(f.id)}>Close</button></td>
            </tr>
          {/each}
        </tbody>
      </table>
      <form class="row new-forward" on:submit|preventDefault={openForward}>
        <select bind:value={hostName}>
          <option value="">Host…</option>
          {#each $hosts as h (h.id)}
            <option value={h.item.name}>{h.item.name}</option>
          {/each}
        </select>
        <select bind:value={kind}>
          <option value="local">Local</option>
          <option value="remote">Remote</option>
        </select>
        <input bind:value={bind} placeholder="bind (127.0.0.1:8080)" />
        <input bind:value={dest} placeholder="destination (host:port)" />
        <button class="primary" type="submit" disabled={busy}>Open</button>
      </form>
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
    width: 620px;
  }
  .new-forward {
    margin-top: 10px;
    flex-wrap: wrap;
  }
  .error {
    color: var(--danger);
    font-size: 13px;
  }
  .actions {
    margin-top: 14px;
  }
</style>
