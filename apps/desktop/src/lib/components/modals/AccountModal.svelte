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
  import { serverErrorCode } from "../../serverError";

  /** Signed out: sign in to an existing account, or create one. */
  let mode: "signin" | "signup" = "signin";
  let server = "";
  let email = "";
  let password = "";
  let password2 = "";
  let inviteCode = "";
  /** Shown only once the server says this account uses two-factor sign-in. */
  let totpNeeded = false;
  let totp = "";
  let totpInput: HTMLInputElement | undefined;
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
    password = "";
    password2 = "";
    totp = "";
    totpNeeded = false;
  }

  function setMode(m: typeof mode): void {
    mode = m;
    error = "";
    totp = "";
    totpNeeded = false;
  }

  /** Friendly text for the sync server's error codes; else the raw message. */
  function accountError(err: unknown): string {
    switch (serverErrorCode(err)) {
      case "totp_invalid":
        return $t("acct.totpWrong");
      case "invite_required":
      case "invalid_invite":
        return $t("acct.inviteRequired");
      case "signups_disabled":
        return $t("acct.signupsDisabled");
      default:
        return errorText(err, $t("toast.error"));
    }
  }

  async function signup(): Promise<void> {
    if (password !== password2) {
      error = $t("acct.passwordMismatch");
      return;
    }
    busy = true;
    error = "";
    try {
      await rpc.accountSignup({ server, email, password, invite_code: inviteCode.trim() || null });
      await refreshAccountStatus();
      password = password2 = "";
    } catch (err) {
      error = accountError(err);
    } finally {
      busy = false;
    }
  }

  async function login(): Promise<void> {
    busy = true;
    error = "";
    try {
      await rpc.accountLogin({ server, email, password, totp: totpNeeded ? totp.trim() || null : null });
      await refreshAccountStatus();
      password = totp = "";
      totpNeeded = false;
    } catch (err) {
      if (serverErrorCode(err) === "totp_required") {
        // Ask for the code only now: most accounts never need it.
        totpNeeded = true;
        error = "";
        setTimeout(() => totpInput?.focus(), 0);
      } else {
        error = accountError(err);
      }
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
        <div class="seg" role="tablist">
          <button role="tab" aria-selected={mode === "signin"} class:on={mode === "signin"} on:click={() => setMode("signin")}>{$t("acct.tabSignin")}</button>
          <button role="tab" aria-selected={mode === "signup"} class:on={mode === "signup"} on:click={() => setMode("signup")}>{$t("acct.tabSignup")}</button>
        </div>
        <p class="hint">{mode === "signin" ? $t("acct.signinHint") : $t("acct.signupHint")}</p>
        <form on:submit|preventDefault={mode === "signin" ? login : signup}>
          <div class="field">
            <label for="acct-server">{$t("acct.server")}</label>
            <div class="input"><input id="acct-server" bind:value={server} placeholder="https://sync.example.com" autocomplete="url" /></div>
          </div>
          <div class="field">
            <label for="acct-email">{$t("acct.email")}</label>
            <div class="input"><input id="acct-email" type="email" bind:value={email} autocomplete="username" /></div>
          </div>
          <div class="field">
            <label for="acct-pass">{mode === "signup" ? $t("acct.masterPassword") : $t("acct.password")}</label>
            <div class="input">
              <input id="acct-pass" type="password" bind:value={password} autocomplete={mode === "signup" ? "new-password" : "current-password"} />
            </div>
            {#if mode === "signup"}<small class="hint">{$t("acct.masterPasswordHint")}</small>{/if}
          </div>
          {#if mode === "signup"}
            <div class="field">
              <label for="acct-pass2">{$t("acct.confirmPassword")}</label>
              <div class="input"><input id="acct-pass2" type="password" bind:value={password2} autocomplete="new-password" /></div>
            </div>
            <div class="field">
              <label for="acct-invite">{$t("acct.inviteCode")}</label>
              <div class="input"><input id="acct-invite" bind:value={inviteCode} placeholder={$t("acct.optional")} autocomplete="off" spellcheck="false" /></div>
              <small class="hint">{$t("acct.inviteHint")}</small>
            </div>
          {:else if totpNeeded}
            <p class="hint">{$t("acct.totpNeeded")}</p>
            <div class="field">
              <label for="acct-totp">{$t("acct.totpLogin")}</label>
              <div class="input">
                <input id="acct-totp" bind:this={totpInput} bind:value={totp} placeholder="000000" inputmode="numeric" autocomplete="one-time-code" maxlength="8" />
              </div>
            </div>
          {/if}
          {#if error}<p class="error-text">{error}</p>{/if}
          <div class="row">
            <span class="spacer"></span>
            {#if mode === "signin"}
              <button class="btn btn-primary" type="submit" disabled={busy || (totpNeeded && !totp.trim())}>{$t("acct.login")}</button>
            {:else}
              <button class="btn btn-primary" type="submit" disabled={busy || !password || !password2}>{$t("acct.signup")}</button>
            {/if}
          </div>
        </form>
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
