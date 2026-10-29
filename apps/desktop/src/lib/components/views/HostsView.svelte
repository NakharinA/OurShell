<!-- Hosts (design `data-view="hosts"`): quick-connect search, group browsing,
     host cards with hover/context actions. -->
<script lang="ts">
  import { open as openDialog } from "@tauri-apps/plugin-dialog";
  import { readFile } from "@tauri-apps/plugin-fs";
  import Icon from "../../icons/Icon.svelte";
  import Menu from "../ui/Menu.svelte";
  import { t, format } from "../../i18n";
  import { groups, hosts, refreshAllItems, refreshHosts } from "../../stores/items";
  import { teams, vaultNames } from "../../stores/teams";
  import {
    hostEditor,
    groupEditor,
    hostsGroupFilter,
    quickConnectFocusToken,
    runSnippetTarget,
    searchQuery,
    shareHostModal,
  } from "../../stores/ui";
  import { openHostTab } from "../../stores/terminal";
  import { focusTerminalTab, focusSftpTab } from "../../stores/windowTabs";
  import { openSftpTab } from "../../stores/sftp";
  import { rpc } from "../../rpc";
  import { toast, errorText } from "../../stores/toast";
  import { parseQuickConnect } from "../../quickConnect";
  import type { Group, Host, ItemRecord } from "../../types";

  let searchInput: HTMLInputElement;
  let menu: { kind: "new" | "ctx" | "move"; x: number; y: number; host?: ItemRecord<Host> } | null = null;

  $: currentGroup = $hostsGroupFilter ? $groups.find((g) => g.id === $hostsGroupFilter) ?? null : null;
  $: childGroups = $groups.filter((g) => (g.item.parent_id ?? null) === ($hostsGroupFilter ?? null));
  $: visibleHosts = $hosts.filter((h) => {
    if ((h.item.group_id ?? null) !== ($hostsGroupFilter ?? null)) return false;
    const q = $searchQuery.trim().toLowerCase();
    return !q || h.item.name.toLowerCase().includes(q) || h.item.hostname.toLowerCase().includes(q);
  });

  quickConnectFocusToken.subscribe((n) => {
    if (n > 0) setTimeout(() => searchInput?.focus(), 30);
  });

  function connect(h: ItemRecord<Host>): void {
    const id = openHostTab(h.id, h.item.name);
    focusTerminalTab(id);
  }

  function openMenuAt(kind: "new", e: MouseEvent): void;
  function openMenuAt(kind: "ctx", e: MouseEvent, host: ItemRecord<Host>): void;
  function openMenuAt(kind: "new" | "ctx", e: MouseEvent, host?: ItemRecord<Host>): void {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    menu = { kind, x: r.right, y: r.bottom + 6, host };
  }

  function openCtxAtPoint(e: MouseEvent, host: ItemRecord<Host>): void {
    e.preventDefault();
    menu = { kind: "ctx", x: e.clientX, y: e.clientY, host };
  }

  function doConnect(): void {
    if (menu?.host) connect(menu.host);
    menu = null;
  }
  function doSftp(): void {
    if (!menu?.host) return;
    const id = openSftpTab(menu.host);
    focusSftpTab(id);
    menu = null;
  }
  function doEdit(h: ItemRecord<Host>): void {
    hostEditor.set({ mode: "edit", record: h });
    menu = null;
  }
  function doRunSnippet(): void {
    if (menu?.host) runSnippetTarget.set({ kind: "host", host: menu.host });
    menu = null;
  }
  async function doDuplicate(h: ItemRecord<Host>): Promise<void> {
    menu = null;
    try {
      const copy = { ...h.item, name: `${h.item.name}-copy` };
      await rpc.itemPut<Host>({ kind: "host", id: null, item: copy });
      await refreshHosts();
      toast.success($t("toast.hostSaved"));
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    }
  }
  function doMove(): void {
    if (menu?.host) menu = { kind: "move", x: menu.x, y: menu.y, host: menu.host };
  }
  async function moveTo(groupId: string | null): Promise<void> {
    const host = menu?.host;
    menu = null;
    if (!host) return;
    try {
      await rpc.itemPut<Host>({ kind: "host", id: host.id, item: { ...host.item, group_id: groupId } });
      await refreshHosts();
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    }
  }
  function doCopyAddr(h: ItemRecord<Host>): void {
    const addr = h.item.username ? `${h.item.username}@${h.item.hostname}` : h.item.hostname;
    void navigator.clipboard.writeText(addr).then(() => toast.success($t("toast.copied")));
    menu = null;
  }
  function doShare(h: ItemRecord<Host>): void {
    shareHostModal.set(h);
    menu = null;
  }
  async function doRemove(h: ItemRecord<Host>): Promise<void> {
    menu = null;
    if (!confirm(`${$t("a.remove")} "${h.item.name}"?`)) return;
    try {
      await rpc.itemDelete({ kind: "host", id: h.id });
      await refreshHosts();
      toast.success($t("toast.hostDeleted"));
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    }
  }

  function openGroup(g: ItemRecord<Group>): void {
    hostsGroupFilter.set(g.id);
  }
  function goUp(): void {
    const parent = currentGroup?.item.parent_id ?? null;
    hostsGroupFilter.set(parent);
  }

  function onConnectClick(): void {
    const result = parseQuickConnect($searchQuery, $hosts.map((h) => h.item.name));
    if (result.type === "connect") {
      const h = $hosts.find((x) => x.item.name === result.hostName);
      if (h) connect(h);
    } else if (result.type === "new") {
      hostEditor.set({
        mode: "new",
        groupId: $hostsGroupFilter,
        prefill: {
          username: result.username ?? undefined,
          hostname: result.hostname,
          port: result.port ?? undefined,
        } as Partial<Host>,
      });
    } else if (result.type === "none") {
      toast.error(format($t("qc.notFound"), { q: result.query }));
    }
  }

  async function importFile(kind: "ssh_config" | "termius_csv"): Promise<void> {
    menu = null;
    try {
      const path = await openDialog({ multiple: false });
      if (!path || Array.isArray(path)) return;
      const bytes = await readFile(path);
      const contents = new TextDecoder().decode(bytes);
      const report =
        kind === "ssh_config"
          ? await rpc.importSshConfig({ contents })
          : await rpc.importTermiusCsv({ contents });
      await refreshAllItems();
      toast.success(
        format($t("import.summary"), {
          hosts: report.hosts_added,
          keys: report.keys_added,
          identities: report.identities_added,
          groups: report.groups_added,
        }),
      );
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    }
  }

  function newGroupHere(): void {
    menu = null;
    groupEditor.set({ mode: "new", parentId: $hostsGroupFilter });
  }

  let focusedHost: ItemRecord<Host> | null = null;

  function keydown(e: KeyboardEvent): void {
    if (menu) {
      if (e.key === "Escape") menu = null;
      return;
    }
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "n") {
      e.preventDefault();
      hostEditor.set({ mode: "new", groupId: $hostsGroupFilter });
    } else if (focusedHost && (e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "e") {
      e.preventDefault();
      doEdit(focusedHost);
    } else if (focusedHost && e.key === "Delete") {
      e.preventDefault();
      void doRemove(focusedHost);
    }
  }
</script>

<svelte:window on:keydown={keydown} />

<section class="view">
  <div class="card-head">
    <div class="title-row">
      <h1 class="card-title">{$t("hosts.title")}</h1>
      <span class="badge accent">{$hosts.length}</span>
      <span class="spacer"></span>
      <label class="search">
        <Icon name="search" />
        <input
          bind:this={searchInput}
          bind:value={$searchQuery}
          placeholder={$t("hdr.search")}
          on:keydown={(e) => e.key === "Enter" && onConnectClick()}
        />
        <button class="btn btn-soft btn-sm" on:click={onConnectClick}>{$t("hdr.connect")}</button>
      </label>
    </div>
    <div class="toolbar">
      <div class="split">
        <button class="btn btn-primary" on:click={() => hostEditor.set({ mode: "new", groupId: $hostsGroupFilter })}>
          <Icon name="plus" />
          <span>{$t("hosts.new")}</span>
        </button>
        <button class="btn btn-primary" aria-label="More" on:click={(e) => openMenuAt("new", e)}>
          <Icon name="chev" />
        </button>
      </div>
      {#if currentGroup}
        <button class="btn btn-ghost" on:click={goUp}>
          <Icon name="cright" class="flip" />
          <span>{$t("hosts.groupBack")}</span>
        </button>
        <span class="badge">{currentGroup.item.name}</span>
      {/if}
      <div class="spacer"></div>
    </div>
  </div>
  <div class="card-body">
    {#if visibleHosts.length === 0 && childGroups.length === 0}
      <div class="empty">
        <Icon name="hosts" size="lg" />
        <strong>{$t("hosts.empty")}</strong>
        <p>{$t("hosts.emptyHint")}</p>
      </div>
    {:else}
      <div class="grid">
        {#each childGroups as g (g.id)}
          <div
            class="item"
            role="button"
            tabindex="0"
            on:click={() => openGroup(g)}
            on:keydown={(e) => e.key === "Enter" && openGroup(g)}
          >
            <span class="round orange"><Icon name="group" /></span>
            <span class="meta">
              <div class="item-title">{g.item.name}</div>
              <div class="item-sub">{$hosts.filter((h) => h.item.group_id === g.id).length} hosts</div>
            </span>
          </div>
        {/each}
        {#each visibleHosts as h (h.id)}
          <div
            class="item"
            role="button"
            tabindex="0"
            on:click={() => connect(h)}
            on:keydown={(e) => e.key === "Enter" && connect(h)}
            on:contextmenu={(e) => openCtxAtPoint(e, h)}
            on:focus={() => (focusedHost = h)}
            on:mouseenter={() => (focusedHost = h)}
          >
            <span class="round"><Icon name="hosts" /></span>
            <span class="meta">
              <div class="item-title">{h.item.name}</div>
              <div class="item-sub">
                {h.item.username ? `${h.item.username}@` : ""}{h.item.hostname}
                {#if h.item.tags.length}· {h.item.tags.join(", ")}{/if}
              </div>
            </span>
            {#if h.vault_id && $vaultNames.get(h.vault_id)}
              <span class="badge accent">{$vaultNames.get(h.vault_id)}</span>
            {/if}
            <span class="item-actions">
              <button class="icon-btn sm" title={$t("a.edit")} on:click|stopPropagation={() => doEdit(h)}>
                <Icon name="pencil" />
              </button>
              <button class="icon-btn sm" aria-label="More" on:click|stopPropagation={(e) => openMenuAt("ctx", e, h)}>
                <Icon name="dots" />
              </button>
            </span>
          </div>
        {/each}
      </div>
      <p class="hint" style="margin-top:18px">{$t("hosts.tip")}</p>
    {/if}
  </div>
</section>

{#if menu?.kind === "new"}
  <Menu x={menu.x} y={menu.y} align="right" onClose={() => (menu = null)}>
    <button class="menu-item" on:click={() => { hostEditor.set({ mode: "new", groupId: $hostsGroupFilter }); menu = null; }}>
      <Icon name="hosts" /><span>{$t("mn.host")}</span>
    </button>
    <button class="menu-item" on:click={newGroupHere}>
      <Icon name="group" /><span>{$t("mn.group")}</span>
    </button>
    <div class="menu-sep"></div>
    <div class="menu-label">{$t("mn.import")}</div>
    <button class="menu-item" on:click={() => importFile("ssh_config")}>
      <Icon name="file" /><span>{$t("mn.sshcfg")}</span>
    </button>
    <button class="menu-item" on:click={() => importFile("termius_csv")}>
      <Icon name="file" /><span>{$t("mn.csv")}</span>
    </button>
  </Menu>
{:else if menu?.kind === "ctx" && menu.host}
  <Menu x={menu.x} y={menu.y} onClose={() => (menu = null)}>
    <button class="menu-item" on:click={doConnect}>
      <Icon name="terminal" /><span>{$t("hdr.connect")}</span><span class="kbd">Enter</span>
    </button>
    <button class="menu-item" on:click={doSftp}>
      <Icon name="folder" /><span>{$t("c.sftp")}</span>
    </button>
    <button class="menu-item" on:click={doRunSnippet}>
      <Icon name="code" /><span>{$t("c.runSnip")}</span>
    </button>
    <div class="menu-sep"></div>
    <button class="menu-item" on:click={() => menu?.host && doEdit(menu.host)}>
      <Icon name="pencil" /><span>{$t("a.edit")}</span><span class="kbd">Ctrl E</span>
    </button>
    <button class="menu-item" on:click={() => menu?.host && doDuplicate(menu.host)}>
      <Icon name="copy" /><span>{$t("a.dup")}</span>
    </button>
    <button class="menu-item" on:click={doMove}>
      <Icon name="move" /><span>{$t("c.move")}</span>
    </button>
    <button class="menu-item" on:click={() => menu?.host && doCopyAddr(menu.host)}>
      <Icon name="link" /><span>{$t("c.copyAddr")}</span>
    </button>
    {#if $teams.filter((v) => v.kind === "team").length > 0}
      <button class="menu-item" on:click={() => menu?.host && doShare(menu.host)}>
        <Icon name="users" /><span>{$t("c.shareTeam")}</span>
      </button>
    {/if}
    <div class="menu-sep"></div>
    <button class="menu-item danger" on:click={() => menu?.host && doRemove(menu.host)}>
      <Icon name="trash" /><span>{$t("a.remove")}</span><span class="kbd">Del</span>
    </button>
  </Menu>
{:else if menu?.kind === "move"}
  <Menu x={menu.x} y={menu.y} onClose={() => (menu = null)}>
    <div class="menu-label">{$t("h.group")}</div>
    <button class="menu-item" on:click={() => moveTo(null)}>
      <Icon name="hosts" /><span>{$t("h.none")}</span>
    </button>
    {#each $groups as g (g.id)}
      <button class="menu-item" on:click={() => moveTo(g.id)}>
        <Icon name="group" /><span>{g.item.name}</span>
      </button>
    {/each}
  </Menu>
{/if}

<style>
  :global(.flip) {
    transform: scaleX(-1);
  }
</style>
