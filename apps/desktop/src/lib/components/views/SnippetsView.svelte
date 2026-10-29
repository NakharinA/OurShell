<!-- Snippets (design `data-view="snippets"`). -->
<script lang="ts">
  import Icon from "../../icons/Icon.svelte";
  import Menu from "../ui/Menu.svelte";
  import { t } from "../../i18n";
  import { rpc } from "../../rpc";
  import { refreshSnippets, snippets } from "../../stores/items";
  import { snippetEditor } from "../../stores/ui";
  import { runSnippetInPaneWhenReady, tabs } from "../../stores/terminal";
  import { focusTerminalTab } from "../../stores/windowTabs";
  import { toast, errorText } from "../../stores/toast";
  import type { ItemRecord, Snippet } from "../../types";

  let menu: { x: number; y: number; record: ItemRecord<Snippet> } | null = null;

  function openMenu(e: MouseEvent, record: ItemRecord<Snippet>): void {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    menu = { x: r.right, y: r.bottom + 6, record };
  }

  function edit(rec: ItemRecord<Snippet>): void {
    menu = null;
    snippetEditor.set({ mode: "edit", record: rec });
  }

  async function duplicate(rec: ItemRecord<Snippet>): Promise<void> {
    menu = null;
    try {
      await rpc.itemPut<Snippet>({ kind: "snippet", id: null, item: { ...rec.item, name: `${rec.item.name}-copy` } });
      await refreshSnippets();
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    }
  }

  async function remove(rec: ItemRecord<Snippet>): Promise<void> {
    menu = null;
    if (!confirm(`${$t("a.remove")} "${rec.item.name}"?`)) return;
    try {
      await rpc.itemDelete({ kind: "snippet", id: rec.id });
      await refreshSnippets();
      toast.success($t("toast.snippetDeleted"));
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    }
  }

  async function runInTerminal(rec: ItemRecord<Snippet>): Promise<void> {
    menu = null;
    const active = $tabs[$tabs.length - 1];
    if (active) {
      focusTerminalTab(active.id);
      const ok = await runSnippetInPaneWhenReady(active.activePaneId, rec.item.script, 3000);
      if (!ok) toast.error($t("s.openTerminalFirst"));
      return;
    }
    toast.error($t("s.openTerminalFirst"));
  }
</script>

<section class="view">
  <div class="card-head">
    <div class="title-row">
      <h1 class="card-title">{$t("snip.title")}</h1>
      <span class="badge accent">{$snippets.length}</span>
    </div>
    <div class="toolbar">
      <button class="btn btn-primary" on:click={() => snippetEditor.set({ mode: "new" })}>
        <Icon name="plus" /><span>{$t("snip.new")}</span>
      </button>
      <div class="spacer"></div>
    </div>
  </div>
  <div class="card-body">
    {#if $snippets.length === 0}
      <div class="empty">
        <Icon name="code" size="lg" />
        <strong>{$t("snip.empty")}</strong>
        <p>{$t("snip.emptyHint")}</p>
      </div>
    {:else}
      <div class="grid wide">
        {#each $snippets as rec (rec.id)}
          <div class="item" role="button" tabindex="0">
            <span class="round"><Icon name="code" /></span>
            <span class="meta">
              <div class="item-title">{rec.item.name}</div>
              <div class="item-sub mono">{rec.item.script.split("\n")[0]}</div>
            </span>
            <span class="item-actions">
              <button class="icon-btn sm" title={$t("a.run")} on:click|stopPropagation={() => runInTerminal(rec)}>
                <Icon name="play" />
              </button>
              <button class="icon-btn sm" aria-label="More" on:click|stopPropagation={(e) => openMenu(e, rec)}>
                <Icon name="dots" />
              </button>
            </span>
          </div>
        {/each}
      </div>
    {/if}
  </div>
</section>

{#if menu}
  <Menu x={menu.x} y={menu.y} align="right" onClose={() => (menu = null)}>
    <button class="menu-item" on:click={() => menu && runInTerminal(menu.record)}>
      <Icon name="play" /><span>{$t("snip.runInTerminal")}</span>
    </button>
    <div class="menu-sep"></div>
    <button class="menu-item" on:click={() => menu && edit(menu.record)}>
      <Icon name="pencil" /><span>{$t("a.edit")}</span>
    </button>
    <button class="menu-item" on:click={() => menu && duplicate(menu.record)}>
      <Icon name="copy" /><span>{$t("a.dup")}</span>
    </button>
    <div class="menu-sep"></div>
    <button class="menu-item danger" on:click={() => menu && remove(menu.record)}>
      <Icon name="trash" /><span>{$t("a.remove")}</span>
    </button>
  </Menu>
{/if}
