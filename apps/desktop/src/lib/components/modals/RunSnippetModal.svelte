<!-- "Run snippet..." picker (design `c.runSnip`): from a host's context menu
     it opens a new terminal tab and types the script in once connected; from
     an already-open terminal pane's overlay it writes straight into it. -->
<script lang="ts">
  import Modal from "../ui/Modal.svelte";
  import Icon from "../../icons/Icon.svelte";
  import { t } from "../../i18n";
  import { snippets } from "../../stores/items";
  import { runSnippetTarget } from "../../stores/ui";
  import { openHostTab, runSnippetInPaneWhenReady, tabs } from "../../stores/terminal";
  import { focusTerminalTab } from "../../stores/windowTabs";
  import { toast } from "../../stores/toast";

  $: target = $runSnippetTarget;

  function close(): void {
    runSnippetTarget.set(null);
  }

  async function run(script: string): Promise<void> {
    const t2 = target;
    close();
    if (!t2) return;
    if (t2.kind === "pane") {
      const ok = await runSnippetInPaneWhenReady(t2.paneId, script, 2000);
      if (!ok) toast.error($t("s.openTerminalFirst"));
      return;
    }
    const tabId = openHostTab(t2.host.id, t2.host.item.name);
    focusTerminalTab(tabId);
    const tab = $tabs.find((x) => x.id === tabId);
    if (!tab) return;
    const ok = await runSnippetInPaneWhenReady(tab.activePaneId, script);
    if (!ok) toast.error($t("s.openTerminalFirst"));
  }
</script>

{#if target}
  <Modal title={$t("c.runSnip")} onClose={close}>
    <div class="psec">
      {#if $snippets.length === 0}
        <p class="hint">{$t("snip.empty")}</p>
      {:else}
        <div class="grid">
          {#each $snippets as rec (rec.id)}
            <button class="item" style="text-align:left" on:click={() => run(rec.item.script)}>
              <span class="round"><Icon name="code" /></span>
              <span class="meta">
                <div class="item-title">{rec.item.name}</div>
                <div class="item-sub mono">{rec.item.script.split("\n")[0]}</div>
              </span>
            </button>
          {/each}
        </div>
      {/if}
    </div>
    <svelte:fragment slot="footer">
      <button class="btn btn-ghost" on:click={close}>{$t("a.cancel")}</button>
    </svelte:fragment>
  </Modal>
{/if}
