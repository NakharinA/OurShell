<script lang="ts">
  import { onMount } from "svelte";
  import { refreshAllItems } from "../stores/items";
  import { startAiWatcher } from "../stores/ai";
  import Sidebar from "./Sidebar.svelte";
  import TopBanner from "./TopBanner.svelte";
  import UpdateBanner from "./UpdateBanner.svelte";
  import TerminalWorkspace from "./terminal/TerminalWorkspace.svelte";
  import HostEditor from "./HostEditor.svelte";
  import IdentitiesPage from "./IdentitiesPage.svelte";
  import KeysPage from "./KeysPage.svelte";
  import SnippetsPanel from "./SnippetsPanel.svelte";
  import ImportDialog from "./ImportDialog.svelte";
  import SftpBrowser from "./SftpBrowser.svelte";
  import ForwardManager from "./ForwardManager.svelte";
  import AiPanel from "./AiPanel.svelte";
  import DangerousModeDialog from "./DangerousModeDialog.svelte";
  import AuditLogViewer from "./AuditLogViewer.svelte";

  onMount(() => {
    startAiWatcher();
    void refreshAllItems();
  });
</script>

<div class="layout">
  <TopBanner />
  <UpdateBanner />
  <div class="body">
    <Sidebar />
    <main class="content">
      <TerminalWorkspace />
    </main>
  </div>
</div>

<HostEditor />
<IdentitiesPage />
<KeysPage />
<SnippetsPanel />
<ImportDialog />
<SftpBrowser />
<ForwardManager />
<AiPanel />
<DangerousModeDialog />
<AuditLogViewer />

<style>
  .layout {
    display: flex;
    flex-direction: column;
    height: 100%;
  }
  .body {
    flex: 1;
    display: flex;
    min-height: 0;
  }
  .content {
    flex: 1;
    min-width: 0;
  }
</style>
