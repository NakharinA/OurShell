<script lang="ts">
  import { rpc } from "../rpc";
  import { refreshVaultStatus } from "../stores/vault";
  import { autofocus } from "../actions/autofocus";

  let password = "";
  let busy = false;
  let error = "";

  async function submit(): Promise<void> {
    if (!password) return;
    busy = true;
    error = "";
    try {
      await rpc.vaultUnlock({ password });
      await refreshVaultStatus();
      password = "";
    } catch (err) {
      error = (err as Error).message ?? "Could not unlock the vault.";
    } finally {
      busy = false;
    }
  }
</script>

<div class="screen">
  <form class="panel card" on:submit|preventDefault={submit}>
    <h1>OurShell</h1>
    <p class="text-dim">Enter your master password to unlock the vault.</p>
    <label>
      Master password
      <input type="password" bind:value={password} use:autofocus />
    </label>
    {#if error}
      <p class="error">{error}</p>
    {/if}
    <button class="primary" type="submit" disabled={busy || !password}>
      {busy ? "Unlocking…" : "Unlock"}
    </button>
  </form>
</div>

<style>
  .screen {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .card {
    width: 360px;
    padding: 28px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .error {
    color: var(--danger);
    font-size: 13px;
  }
</style>
