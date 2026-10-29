<!--
  Custom frameless title bar (Termius style, design: `.titlebar`): a pinned
  "Vaults" home tab, one tab per open session (terminal or SFTP), a "+" that
  jumps to Hosts and focuses quick-connect, and window controls. Dragging the
  window uses `data-tauri-drag-region`; minimize/maximize/close use
  `@tauri-apps/api/window` (capabilities: core:window:allow-minimize /
  allow-toggle-maximize / allow-close / allow-start-dragging).
-->
<script lang="ts">
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import Icon from "../icons/Icon.svelte";
  import { t } from "../i18n";
  import {
    activeWindowTab,
    closeWindowTab,
    focusSftpTab,
    focusTerminalTab,
    focusVaultsTab,
    windowTabList,
  } from "../stores/windowTabs";
  import { requestQuickConnectFocus } from "../stores/ui";
  import { anyDangerous } from "../stores/ai";

  const win = getCurrentWindow();

  function select(ref: { type: "terminal" | "sftp"; id: string }): void {
    if (ref.type === "terminal") focusTerminalTab(ref.id);
    else focusSftpTab(ref.id);
  }

  function close(ref: { type: "terminal" | "sftp"; id: string }, e: MouseEvent): void {
    e.stopPropagation();
    closeWindowTab(ref);
  }
</script>

<header class="titlebar" data-tauri-drag-region>
  <button
    class="wtab"
    class:on={$activeWindowTab.type === "vaults"}
    on:click={focusVaultsTab}
  >
    <Icon name="vault" size="sm" />
    <span class="label">{$t("tab.vaults")}</span>
    {#if $anyDangerous}
      <Icon name="warn" size="sm" class="muted" />
    {/if}
  </button>

  {#each $windowTabList as tab (tab.id)}
    <button
      class="wtab"
      class:on={$activeWindowTab.type !== "vaults" && $activeWindowTab.id === tab.id}
      on:click={() => select(tab)}
    >
      <Icon name={tab.type === "sftp" ? "folder" : "terminal"} size="sm" />
      <span class="label">{tab.title}</span>
      <span
        class="tab-x"
        role="button"
        tabindex="0"
        aria-label="Close"
        on:click={(e) => close(tab, e)}
        on:keydown={(e) => e.key === "Enter" && close(tab, e as unknown as MouseEvent)}
      >
        <Icon name="x" size="sm" />
      </span>
    </button>
  {/each}

  <button class="win" title={$t("tab.new")} aria-label={$t("tab.new")} on:click={requestQuickConnectFocus}>
    <Icon name="plus" size="sm" />
  </button>

  <div class="spacer"></div>

  <button class="win" aria-label="Minimize" on:click={() => win.minimize()}>
    <Icon name="min" size="sm" />
  </button>
  <button class="win" aria-label="Maximize" on:click={() => win.toggleMaximize()}>
    <Icon name="max" size="sm" />
  </button>
  <button class="win close" aria-label="Close" on:click={() => win.close()}>
    <Icon name="x" size="sm" />
  </button>
</header>
