<!--
  Content of the pinned "Vaults" tab: sidebar + the active nav view inside a
  floating card (design: `.app-body` > `.sidebar` + `.main` > `.page` >
  `.card`). All the modals it can open (host/key/forward/snippet editors,
  account, teams, import, dangerous mode) are mounted once here too.
-->
<script lang="ts">
  import { onMount } from "svelte";
  import Sidebar from "./Sidebar.svelte";
  import HostsView from "./views/HostsView.svelte";
  import KeychainView from "./views/KeychainView.svelte";
  import ForwardingView from "./views/ForwardingView.svelte";
  import SnippetsView from "./views/SnippetsView.svelte";
  import McpAiView from "./views/McpAiView.svelte";
  import LogsView from "./views/LogsView.svelte";
  import HostModal from "./modals/HostModal.svelte";
  import GroupModal from "./modals/GroupModal.svelte";
  import KeyModal from "./modals/KeyModal.svelte";
  import ForwardModal from "./modals/ForwardModal.svelte";
  import SnippetModal from "./modals/SnippetModal.svelte";
  import AccountModal from "./modals/AccountModal.svelte";
  import TeamCreateModal from "./modals/TeamCreateModal.svelte";
  import TeamMembersModal from "./modals/TeamMembersModal.svelte";
  import DangerousModeModal from "./modals/DangerousModeModal.svelte";
  import RunSnippetModal from "./modals/RunSnippetModal.svelte";
  import ShareHostModal from "./modals/ShareHostModal.svelte";
  import { activeView, sidebarMini } from "../stores/ui";
  import { refreshAllItems } from "../stores/items";
  import { startAiWatcher } from "../stores/ai";

  onMount(() => {
    startAiWatcher();
    void refreshAllItems();
  });
</script>

<div class="app-body" class:mini={$sidebarMini}>
  <Sidebar />
  <div class="main">
    <div class="page">
      <main class="card" style="min-width:0;min-height:0;overflow:hidden">
        {#if $activeView === "hosts"}
          <HostsView />
        {:else if $activeView === "keychain"}
          <KeychainView />
        {:else if $activeView === "forwarding"}
          <ForwardingView />
        {:else if $activeView === "snippets"}
          <SnippetsView />
        {:else if $activeView === "mcp"}
          <McpAiView />
        {:else if $activeView === "logs"}
          <LogsView />
        {/if}
      </main>
    </div>
  </div>
</div>

<HostModal />
<GroupModal />
<KeyModal />
<ForwardModal />
<SnippetModal />
<AccountModal />
<TeamCreateModal />
<TeamMembersModal />
<DangerousModeModal />
<RunSnippetModal />
<ShareHostModal />
