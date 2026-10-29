<script lang="ts">
  import Icon from "../icons/Icon.svelte";
  import { t } from "../i18n";
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

  function copyPhrase(): void {
    void navigator.clipboard.writeText(recoveryPhrase);
  }
</script>

<div class="screen">
  {#if step === "form"}
    <form class="card panel auth-card" on:submit|preventDefault={createVault}>
      <div class="brand" style="justify-content:center;padding:0 0 4px">
        <Icon name="logo" />
        <span>OurShell</span>
      </div>
      <h1 style="text-align:center;font-size:18px">{$t("create.title")}</h1>
      <p class="hint" style="text-align:center">{$t("create.sub")}</p>
      <div class="field">
        <label for="create-pass">{$t("create.password")}</label>
        <div class="input"><Icon name="pass" /><input id="create-pass" type="password" bind:value={password} use:autofocus /></div>
      </div>
      <div class="field">
        <label for="create-confirm">{$t("create.confirm")}</label>
        <div class="input"><Icon name="pass" /><input id="create-confirm" type="password" bind:value={confirm} /></div>
      </div>
      {#if mismatch}<p class="error-text">{$t("create.mismatch")}</p>{/if}
      <label class="toggle-row">
        <input type="checkbox" bind:checked={localOnly} style="width:16px;height:16px" />
        <span class="grow">{$t("create.localOnly")}</span>
      </label>
      <div class="alert">
        <Icon name="warn" />
        <span>{$t("create.warning")}</span>
      </div>
      {#if error}<p class="error-text">{error}</p>{/if}
      <button class="btn btn-primary btn-block" type="submit" disabled={busy || !password || mismatch}>
        {busy ? $t("create.creating") : $t("create.submit")}
      </button>
    </form>
  {:else}
    <div class="card panel auth-card">
      <h1 style="text-align:center;font-size:18px">{$t("create.recoveryTitle")}</h1>
      <div class="alert">
        <Icon name="warn" />
        <span>{$t("create.recoveryWarning")}</span>
      </div>
      <div class="code-wrap">
        <pre class="code" style="user-select:all">{recoveryPhrase}</pre>
        <button class="btn btn-soft btn-sm copy-btn" on:click={copyPhrase}>
          <Icon name="copy" /><span>{$t("a.copy")}</span>
        </button>
      </div>
      <label class="toggle-row">
        <input type="checkbox" bind:checked={savedConfirmed} style="width:16px;height:16px" />
        <span class="grow">{$t("create.recoverySaved")}</span>
      </label>
      <button class="btn btn-primary btn-block" disabled={!savedConfirmed || busy} on:click={finish}>
        {busy ? $t("create.opening") : $t("create.continue")}
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
    background: var(--bg-page);
  }
  .auth-card {
    width: 440px;
    padding: 28px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
</style>
