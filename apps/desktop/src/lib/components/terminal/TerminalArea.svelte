<!-- Content of one terminal window tab: the pane tree fills the area under
     the title bar, with a small top-right overlay (SFTP for this host,
     split, snippet runner, disconnect) acting on the active pane. -->
<script lang="ts">
  import Icon from "../../icons/Icon.svelte";
  import { t } from "../../i18n";
  import PaneView from "./PaneView.svelte";
  import { closePane, findLeaf, splitPane, tabs } from "../../stores/terminal";
  import { runSnippetTarget } from "../../stores/ui";
  import { openSftpTab } from "../../stores/sftp";
  import { focusSftpTab, focusVaultsTab } from "../../stores/windowTabs";
  import { hosts } from "../../stores/items";

  export let tabId: string;

  $: tab = $tabs.find((t2) => t2.id === tabId);
  $: activeLeaf = tab ? findLeaf(tab.root, tab.activePaneId) : null;

  function split(orientation: "row" | "col"): void {
    if (!tab || !activeLeaf) return;
    splitPane(tab.id, activeLeaf.id, orientation, activeLeaf.hostId, activeLeaf.hostName);
  }

  function sftp(): void {
    if (!activeLeaf) return;
    const record = $hosts.find((h) => h.id === activeLeaf?.hostId);
    if (!record) return;
    const id = openSftpTab(record);
    focusSftpTab(id);
  }

  function runSnippet(): void {
    if (!activeLeaf) return;
    runSnippetTarget.set({ kind: "pane", paneId: activeLeaf.id, label: activeLeaf.hostName });
  }

  async function disconnect(): Promise<void> {
    if (!tab || !activeLeaf) return;
    const willCloseTab = tab.root.id === activeLeaf.id;
    closePane(tab.id, activeLeaf.id);
    if (willCloseTab) focusVaultsTab();
  }
</script>

{#if tab}
  <div class="term-area">
    <div class="term-overlay">
      <button class="icon-btn sm" title={$t("term.sftp")} on:click={sftp}><Icon name="folder" size="sm" /></button>
      <button class="icon-btn sm" title={$t("term.splitH")} on:click={() => split("row")}><Icon name="split-h" size="sm" /></button>
      <button class="icon-btn sm" title={$t("term.splitV")} on:click={() => split("col")}><Icon name="split-v" size="sm" /></button>
      <button class="icon-btn sm" title={$t("term.runSnippet")} on:click={runSnippet}><Icon name="code" size="sm" /></button>
      <button class="icon-btn sm" title={$t("term.disconnect")} on:click={disconnect}><Icon name="power" size="sm" /></button>
    </div>
    <PaneView tabId={tab.id} node={tab.root} activePaneId={tab.activePaneId} />
  </div>
{:else}
  <div class="empty">
    <Icon name="terminal" size="lg" />
    <strong>{$t("term.closedTab")}</strong>
    <p>{$t("term.closedTabHint")}</p>
  </div>
{/if}

<style>
  .term-area {
    position: relative;
    flex: 1;
    min-height: 0;
  }
  .term-overlay {
    position: absolute;
    top: 10px;
    right: 14px;
    z-index: 20;
    display: flex;
    gap: 4px;
    padding: 4px;
    border-radius: var(--r-pill);
    background: var(--bg-surface);
    box-shadow: var(--shadow-menu);
  }
</style>
