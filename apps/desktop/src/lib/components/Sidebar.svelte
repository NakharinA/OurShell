<!--
  Spike-admin-style sidebar (design: `.sidebar`): brand, vault switcher,
  WORKSPACE / AI / ACTIVITY nav, and a footer with language, theme and
  profile menus. Lives inside the "Vaults" home tab only.
-->
<script lang="ts">
  import Icon from "../icons/Icon.svelte";
  import Menu from "./ui/Menu.svelte";
  import { t, lang, format, type Lang } from "../i18n";
  import { activeView, sidebarMini, accountModalOpen, teamCreateOpen, teamMembersOpen } from "../stores/ui";
  import type { NavView } from "../stores/ui";
  import { refreshTeams, teams } from "../stores/teams";
  import { currentVaultId } from "../stores/vaultContext";
  import { hosts } from "../stores/items";
  import { pendingApprovals } from "../stores/ai";
  import { accountStatus, refreshAccountStatus } from "../stores/account";
  import { rpc } from "../rpc";
  import { refreshVaultStatus } from "../stores/vault";
  import { uiTheme } from "../stores/theme";
  import { toast, errorText } from "../stores/toast";
  import { appVersion, checkForUpdates, loadAppVersion } from "../stores/update";
  import { onMount } from "svelte";

  let menu: "vault" | "lang" | "profile" | null = null;
  let anchorX = 0;
  let anchorY = 0;
  let syncing = false;

  onMount(() => {
    refreshTeams().catch(() => {});
    void refreshAccountStatus();
    void loadAppVersion();
  });

  function checkForUpdatesNow(): void {
    menu = null;
    void checkForUpdates(true);
  }

  function openMenuAt(which: typeof menu, e: MouseEvent): void {
    const btn = e.currentTarget as HTMLElement;
    const r = btn.getBoundingClientRect();
    anchorX = r.left;
    anchorY = r.bottom + 6;
    menu = menu === which ? null : which;
  }

  const navItems: { view: NavView; icon: string; key: string; cap: string }[] = [
    { view: "hosts", icon: "hosts", key: "nav.hosts", cap: "nav.workspace" },
    { view: "keychain", icon: "key", key: "nav.keychain", cap: "" },
    { view: "forwarding", icon: "forward", key: "nav.forwarding", cap: "" },
    { view: "snippets", icon: "code", key: "nav.snippets", cap: "" },
    { view: "mcp", icon: "plug", key: "nav.mcp", cap: "nav.ai" },
    { view: "logs", icon: "history", key: "nav.logs", cap: "nav.activity" },
  ];

  $: vaultLabel = $currentVaultId
    ? $teams.find((v) => v.vault_id === $currentVaultId)?.name ?? $t("vault.personal")
    : $t("vault.personal");

  function pickVault(id: string | null): void {
    currentVaultId.set(id);
    menu = null;
  }

  function setLang(l: Lang): void {
    lang.set(l);
    menu = null;
  }

  async function lock(): Promise<void> {
    menu = null;
    await rpc.vaultLock();
    await refreshVaultStatus();
  }

  async function syncNow(): Promise<void> {
    syncing = true;
    try {
      const report = await rpc.syncRun();
      toast.success(
        `Pushed ${report.pushed}, pulled ${report.pulled}, ${report.conflicts} conflicts.`,
      );
      await refreshAccountStatus();
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    } finally {
      syncing = false;
    }
  }

  async function signOut(): Promise<void> {
    menu = null;
    try {
      await rpc.accountLogout();
      await refreshAccountStatus();
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    }
  }
</script>

<aside class="sidebar">
  <div class="brand">
    <Icon name="logo" />
    <span>OurShell</span>
    <button
      class="icon-btn sm"
      title={$t("hdr.menu")}
      aria-label={$t("hdr.menu")}
      on:click={() => sidebarMini.update((v) => !v)}
    >
      <Icon name="menu" />
    </button>
  </div>

  <div class="side-top">
    <button
      class="vault-card"
      aria-expanded={menu === "vault"}
      title={$t("vault.switch")}
      on:click={(e) => openMenuAt("vault", e)}
    >
      <span class="vault-ic"><Icon name="vault" /></span>
      <span class="grow">
        <small class="vault-cap">{$t("vault.title")}</small>
        <b>{vaultLabel}</b>
        <small>{$t("vault.synced")} · {format($t("vault.itemCount"), { n: $hosts.length })}</small>
      </span>
      <Icon name="chev" size="sm" class="chev" />
    </button>
  </div>

  <nav class="nav">
    {#each navItems as item (item.view)}
      {#if item.cap}
        <div class="nav-cap">
          <Icon name="dots" size="sm" class="dotcap" />
          <span>{$t(item.cap)}</span>
        </div>
      {/if}
      <button
        class="nav-item"
        class:active={$activeView === item.view}
        on:click={() => activeView.set(item.view)}
      >
        <Icon name={item.icon} />
        <span>{$t(item.key)}</span>
        {#if item.view === "mcp" && $pendingApprovals.length > 0}
          <span class="nav-badge">{$pendingApprovals.length}</span>
        {/if}
      </button>
    {/each}
  </nav>

  <div class="side-foot">
    <div class="prefs">
      <button class="lang-btn" aria-expanded={menu === "lang"} aria-label="Language" on:click={(e) => openMenuAt("lang", e)}>
        <Icon name="lang" />
        <span>{$lang === "th" ? "ไทย" : "English"}</span>
        <Icon name="chev" size="sm" class="chev" />
      </button>
      <span class="spacer"></span>
      <button
        class="icon-btn"
        title={$t("hdr.theme")}
        aria-label={$t("hdr.theme")}
        on:click={() => uiTheme.update((v) => (v === "dark" ? "light" : "dark"))}
      >
        <Icon name={$uiTheme === "dark" ? "sun" : "moon"} />
      </button>
    </div>
    <button class="profile" aria-expanded={menu === "profile"} on:click={(e) => openMenuAt("profile", e)}>
      <span class="avatar">{($accountStatus.email ?? "L")[0].toUpperCase()}</span>
      <span class="who-txt">
        <b>{$accountStatus.signed_in ? $accountStatus.email : $t("pm.localVault")}</b>
        {#if $accountStatus.signed_in}<small>{$accountStatus.server}</small>{/if}
      </span>
      <Icon name="dots" size="sm" class="pdots" />
    </button>
  </div>
</aside>

{#if menu === "vault"}
  <Menu x={anchorX} y={anchorY} onClose={() => (menu = null)}>
    <div class="menu-label">{$t("vault.title")}</div>
    <button class="menu-item" class:checked={$currentVaultId === null} on:click={() => pickVault(null)}>
      <Icon name="vault" />
      <span>{$t("vault.personal")}</span>
      <Icon name="check" size="sm" class="check" />
    </button>
    {#each $teams.filter((v) => v.kind === "team") as v (v.vault_id)}
      <div class="menu-item" class:checked={$currentVaultId === v.vault_id} style="gap:4px">
        <button class="menu-item" style="flex:1;padding:0;height:auto" on:click={() => pickVault(v.vault_id)}>
          <Icon name="users" />
          <span style="flex:1">{v.name}</span>
          <Icon name="check" size="sm" class="check" />
        </button>
        <button
          class="icon-btn sm"
          title={$t("vault.manageMembers")}
          on:click={() => {
            teamMembersOpen.set(v.vault_id);
            menu = null;
          }}
        >
          <Icon name="dots" size="sm" />
        </button>
      </div>
    {/each}
    <div class="menu-sep"></div>
    <button
      class="menu-item"
      on:click={() => {
        teamCreateOpen.set(true);
        menu = null;
      }}
    >
      <Icon name="plus" />
      <span>{$t("vault.create")}</span>
    </button>
  </Menu>
{:else if menu === "lang"}
  <Menu x={anchorX} y={anchorY} onClose={() => (menu = null)}>
    <button class="menu-item" class:checked={$lang === "en"} on:click={() => setLang("en")}>
      <span>English</span>
      <Icon name="check" size="sm" class="check" />
    </button>
    <button class="menu-item" class:checked={$lang === "th"} on:click={() => setLang("th")}>
      <span>ไทย</span>
      <Icon name="check" size="sm" class="check" />
    </button>
  </Menu>
{:else if menu === "profile"}
  <Menu x={anchorX} y={anchorY} onClose={() => (menu = null)}>
    <button
      class="menu-item"
      on:click={() => {
        accountModalOpen.set(true);
        menu = null;
      }}
    >
      <Icon name="user" />
      <span>{$t("pm.account")}</span>
    </button>
    {#if $accountStatus.signed_in}
      <button class="menu-item" disabled={syncing} on:click={syncNow}>
        <Icon name="refresh" />
        <span>{$t("pm.syncNow")}</span>
      </button>
    {/if}
    <button class="menu-item" on:click={checkForUpdatesNow}>
      <Icon name="refresh" />
      <span>{$t("pm.checkUpdates")}</span>
    </button>
    <div class="menu-sep"></div>
    <button class="menu-item" on:click={lock}>
      <Icon name="lock" />
      <span>{$t("pm.lock")}</span>
    </button>
    {#if $accountStatus.signed_in}
      <button class="menu-item danger" on:click={signOut}>
        <Icon name="x" />
        <span>{$t("pm.signout")}</span>
      </button>
    {/if}
    <div class="menu-sep"></div>
    <div class="menu-label about">{format($t("pm.about"), { v: $appVersion || "…" })}</div>
  </Menu>
{/if}
