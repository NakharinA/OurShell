<!-- Account & sync (profile menu -> "Account & sync..."). -->
<script lang="ts">
  import Modal from "../ui/Modal.svelte";
  import Icon from "../../icons/Icon.svelte";
  import { t, format } from "../../i18n";
  import { rpc } from "../../rpc";
  import { accountModalOpen } from "../../stores/ui";
  import { accountStatus, refreshAccountStatus } from "../../stores/account";
  import { toast, errorText } from "../../stores/toast";
  import type { DeviceInfo } from "../../types";

  let server = "";
  let email = "";
  let password = "";
  let totp = "";
  let busy = false;
  let error = "";
  let devices: DeviceInfo[] = [];
  let totpSetup: { secret_base32: string; otpauth_url: string } | null = null;
  let totpCode = "";
  let changeOld = "";
  let changeNew = "";

  $: if ($accountModalOpen && $accountStatus.signed_in) void loadDevices();

  async function loadDevices(): Promise<void> {
    try {
      devices = await rpc.accountDevices();
    } catch {
      devices = [];
    }
  }

  function close(): void {
    accountModalOpen.set(false);
    error = "";
    totpSetup = null;
  }

  async function signup(): Promise<void> {
    busy = true;
    error = "";
    try {
      await rpc.accountSignup({ server, email, password });
      await refreshAccountStatus();
    } catch (err) {
      error = errorText(err, $t("toast.error"));
    } finally {
      busy = false;
    }
  }

  async function login(): Promise<void> {
    busy = true;
    error = "";
    try {
      await rpc.accountLogin({ server, email, password, totp: totp || null });
      await refreshAccountStatus();
    } catch (err) {
      error = errorText(err, $t("toast.error"));
    } finally {
      busy = false;
    }
  }

  async function logout(): Promise<void> {
    busy = true;
    try {
      await rpc.accountLogout();
      await refreshAccountStatus();
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    } finally {
      busy = false;
    }
  }

  async function syncNow(): Promise<void> {
    busy = true;
    try {
      const report = await rpc.syncRun();
      toast.success(
        format($t("acct.syncReport"), {
          pushed: report.pushed,
          pulled: report.pulled,
          conflicts: report.conflicts,
        }),
      );
      await refreshAccountStatus();
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    } finally {
      busy = false;
    }
  }

  async function revoke(deviceId: string): Promise<void> {
    try {
      await rpc.accountRevokeDevice({ device_id: deviceId });
      await loadDevices();
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    }
  }

  async function startTotp(): Promise<void> {
    try {
      totpSetup = await rpc.accountTotpSetup();
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    }
  }
  async function confirmTotp(): Promise<void> {
    try {
      await rpc.accountTotpConfirm({ code: totpCode });
      totpSetup = null;
      totpCode = "";
      toast.success($t("a.saved"));
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    }
  }

  async function changePassword(): Promise<void> {
    busy = true;
    error = "";
    try {
      await rpc.accountChangePassword({ old_password: changeOld || null, new_password: changeNew });
      changeOld = "";
      changeNew = "";
      toast.success($t("a.saved"));
    } catch (err) {
      error = errorText(err, $t("toast.error"));
    } finally {
      busy = false;
    }
  }
</script>

{#if $accountModalOpen}
  <Modal title={$t("acct.title")} size="wide" onClose={close}>
    {#if !$accountStatus.signed_in}
      <div class="psec">
        <p class="hint">{$t("acct.signedOutHint")}</p>
        <div class="field">
          <label for="acct-server">{$t("acct.server")}</label>
          <div class="input"><input id="acct-server" bind:value={server} placeholder="https://sync.example.com" /></div>
        </div>
        <div class="field">
          <label for="acct-email">{$t("acct.email")}</label>
          <div class="input"><input id="acct-email" bind:value={email} /></div>
        </div>
        <div class="field">
          <label for="acct-pass">{$t("acct.password")}</label>
          <div class="input"><input id="acct-pass" type="password" bind:value={password} /></div>
        </div>
        <div class="field">
          <label for="acct-totp">TOTP</label>
          <div class="input"><input id="acct-totp" bind:value={totp} placeholder="000000" /></div>
        </div>
        {#if error}<p class="error-text">{error}</p>{/if}
        <div class="row">
          <span class="spacer"></span>
          <button class="btn btn-soft" disabled={busy} on:click={signup}>{$t("acct.signup")}</button>
          <button class="btn btn-primary" disabled={busy} on:click={login}>{$t("acct.login")}</button>
        </div>
      </div>
    {:else}
      <div class="psec">
        <div class="row"><span class="lbl">{$t("acct.email")}</span><span class="spacer"></span><span>{$accountStatus.email}</span></div>
        <div class="row"><span class="lbl">{$t("acct.server")}</span><span class="spacer"></span><span>{$accountStatus.server}</span></div>
        <div class="row">
          <span class="lbl">{$t("acct.lastSync")}</span><span class="spacer"></span>
          <span>{$accountStatus.last_sync ?? $t("acct.never")}</span>
          <button class="btn btn-soft btn-sm" disabled={busy} on:click={syncNow}>{$t("acct.syncNow")}</button>
        </div>
      </div>
      <div class="psec">
        <h3><Icon name="monitor" /><span>{$t("acct.devices")}</span></h3>
        {#each devices as d (d.device_id)}
          <div class="row">
            <span class="grow">{d.name}</span>
            {#if d.revoked}<span class="badge err">revoked</span>{/if}
            <button class="btn btn-ghost btn-sm" on:click={() => revoke(d.device_id)}>{$t("acct.revoke")}</button>
          </div>
        {/each}
      </div>
      <div class="psec">
        <h3><Icon name="shieldok" /><span>{$t("acct.totp")}</span></h3>
        {#if totpSetup}
          <p class="hint">{$t("acct.totpSecret")}: <span class="mono">{totpSetup.secret_base32}</span></p>
          <pre class="code">{totpSetup.otpauth_url}</pre>
          <div class="row">
            <div class="input grow"><input bind:value={totpCode} placeholder={$t("acct.totpCode")} /></div>
            <button class="btn btn-primary" on:click={confirmTotp}>{$t("acct.totpConfirm")}</button>
          </div>
        {:else}
          <button class="btn btn-outline" on:click={startTotp}>{$t("acct.totpSetup")}</button>
        {/if}
      </div>
      <div class="psec">
        <h3><Icon name="pass" /><span>{$t("acct.changePassword")}</span></h3>
        <div class="field">
          <label for="chg-old">{$t("acct.oldPassword")}</label>
          <div class="input"><input id="chg-old" type="password" bind:value={changeOld} /></div>
        </div>
        <div class="field">
          <label for="chg-new">{$t("acct.newPassword")}</label>
          <div class="input"><input id="chg-new" type="password" bind:value={changeNew} /></div>
        </div>
        {#if error}<p class="error-text">{error}</p>{/if}
        <button class="btn btn-primary" style="align-self:flex-start" disabled={busy || !changeNew} on:click={changePassword}>
          {$t("acct.changePassword")}
        </button>
      </div>
      <div class="psec">
        <button class="btn btn-danger" style="align-self:flex-start" disabled={busy} on:click={logout}>{$t("acct.logout")}</button>
      </div>
    {/if}
    <svelte:fragment slot="footer">
      <button class="btn btn-ghost" on:click={close}>{$t("p.close")}</button>
    </svelte:fragment>
  </Modal>
{/if}
