<script lang="ts">
  import Icon from "../icons/Icon.svelte";
  import { t } from "../i18n";
  import { rpc } from "../rpc";
  import { refreshVaultStatus } from "../stores/vault";
  import { autofocus } from "../actions/autofocus";

  let password = "";
  let showPassword = false;
  let busy = false;
  let error = "";
  let recovering = false;
  let phrase = "";
  let newPassword = "";

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

  async function recover(): Promise<void> {
    if (!phrase || !newPassword) return;
    busy = true;
    error = "";
    try {
      await rpc.vaultRecover({ phrase, new_password: newPassword });
      await rpc.vaultUnlock({ password: newPassword });
      await refreshVaultStatus();
    } catch (err) {
      error = (err as Error).message ?? "Could not recover the vault.";
    } finally {
      busy = false;
    }
  }
</script>

<div class="screen">
  <div class="card panel auth-card">
    <div class="brand" style="justify-content:center;padding:0 0 4px">
      <Icon name="logo" />
      <span>{$t("unlock.title")}</span>
    </div>
    {#if !recovering}
      <p class="hint" style="text-align:center">{$t("unlock.sub")}</p>
      <form on:submit|preventDefault={submit} class="col">
        <div class="field">
          <label for="unlock-pass">{$t("unlock.password")}</label>
          <div class="input">
            <Icon name="pass" />
            {#if showPassword}
              <input id="unlock-pass" bind:value={password} use:autofocus />
            {:else}
              <input id="unlock-pass" type="password" bind:value={password} use:autofocus />
            {/if}
            <button type="button" on:click={() => (showPassword = !showPassword)}>
              <Icon name={showPassword ? "eye" : "eyeoff"} size="sm" />
            </button>
          </div>
        </div>
        {#if error}<p class="error-text">{error}</p>{/if}
        <button class="btn btn-primary btn-block" type="submit" disabled={busy || !password}>
          {busy ? $t("unlock.unlocking") : $t("unlock.submit")}
        </button>
      </form>
      <button class="link" style="align-self:center;margin-top:4px" on:click={() => (recovering = true)}>
        {$t("unlock.recover")}
      </button>
    {:else}
      <p class="hint" style="text-align:center">{$t("unlock.recoverTitle")}</p>
      <form on:submit|preventDefault={recover} class="col">
        <div class="field">
          <label for="rec-phrase">{$t("unlock.recoverPhrase")}</label>
          <div class="textarea"><textarea id="rec-phrase" bind:value={phrase}></textarea></div>
        </div>
        <div class="field">
          <label for="rec-newpass">{$t("unlock.recoverNew")}</label>
          <div class="input"><input id="rec-newpass" type="password" bind:value={newPassword} /></div>
        </div>
        {#if error}<p class="error-text">{error}</p>{/if}
        <button class="btn btn-primary btn-block" type="submit" disabled={busy || !phrase || !newPassword}>
          {$t("a.continue")}
        </button>
      </form>
      <button class="link" style="align-self:center;margin-top:4px" on:click={() => (recovering = false)}>
        {$t("a.back")}
      </button>
    {/if}
  </div>
</div>

<style>
  .screen {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--bg-page);
  }
  .auth-card {
    width: 380px;
    padding: 28px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .col {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
</style>
