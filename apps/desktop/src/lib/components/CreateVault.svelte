<script lang="ts">
  import { rpc } from "../rpc";
  import { refreshVaultStatus } from "../stores/vault";
  import { autofocus } from "../actions/autofocus";

  type Step = "form" | "recovery";
  let step: Step = "form";

  let password = "";
  let confirm = "";
  let localOnly = false;
  let busy = false;
  let error = "";
  let recoveryPhrase = "";
  let savedConfirmed = false;

  $: mismatch = confirm.length > 0 && password !== confirm;

  async function createVault(): Promise<void> {
    if (!password || password !== confirm) return;
    busy = true;
    error = "";
    try {
      const result = await rpc.vaultInit({ password, local_only: localOnly });
      recoveryPhrase = result.recovery_phrase;
      step = "recovery";
    } catch (err) {
      error = (err as Error).message ?? "Could not create the vault.";
    } finally {
      busy = false;
    }
  }

  async function finish(): Promise<void> {
    busy = true;
    try {
      // vault.init may or may not leave the vault unlocked; make sure it is.
      try {
        await rpc.vaultUnlock({ password });
      } catch {
        /* already unlocked - fine */
      }
      await refreshVaultStatus();
    } finally {
      busy = false;
    }
  }
</script>

<div class="screen">
  {#if step === "form"}
    <form class="panel card" on:submit|preventDefault={createVault}>
      <h1>Create your vault</h1>
      <p class="text-dim">
        This password encrypts everything locally. OurShell never sends it anywhere.
      </p>
      <label>
        Master password
        <input type="password" bind:value={password} use:autofocus />
      </label>
      <label>
        Confirm password
        <input type="password" bind:value={confirm} />
      </label>
      {#if mismatch}
        <p class="error">Passwords do not match.</p>
      {/if}
      <label class="checkbox">
        <input type="checkbox" bind:checked={localOnly} />
        Local-only vault (never syncs to a server)
      </label>
      <p class="warning">
        If you lose this password <strong>and</strong> the recovery phrase you're about to see,
        your data cannot be recovered. There is no "forgot password" option.
      </p>
      {#if error}
        <p class="error">{error}</p>
      {/if}
      <button class="primary" type="submit" disabled={busy || !password || mismatch}>
        {busy ? "Creating…" : "Create vault"}
      </button>
    </form>
  {:else}
    <div class="panel card recovery">
      <h1>Save your recovery phrase</h1>
      <p class="warning">
        Write this down and store it somewhere safe (not in the vault itself). It is the
        <strong>only</strong> way back in if you forget your master password. OurShell will
        not show it again.
      </p>
      <pre class="phrase">{recoveryPhrase}</pre>
      <label class="checkbox">
        <input type="checkbox" bind:checked={savedConfirmed} />
        I saved my recovery phrase somewhere safe
      </label>
      <button class="primary" disabled={!savedConfirmed || busy} on:click={finish}>
        {busy ? "Opening…" : "Continue"}
      </button>
    </div>
  {/if}
</div>

<style>
  .screen {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 20px;
  }
  .card {
    width: 440px;
    padding: 28px;
    display: flex;
    flex-direction: column;
    gap: 14px;
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
  .warning {
    color: var(--warning);
    font-size: 13px;
    background: rgba(245, 158, 11, 0.1);
    border: 1px solid var(--warning);
    border-radius: var(--radius);
    padding: 10px;
  }
  .phrase {
    font-family: var(--mono);
    background: var(--bg-sunken);
    border: 1px solid var(--panel-border);
    border-radius: var(--radius);
    padding: 14px;
    white-space: pre-wrap;
    word-break: break-word;
    user-select: all;
  }
</style>
