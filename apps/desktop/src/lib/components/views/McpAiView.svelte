<!-- MCP & AI (design `data-view="mcp"`, replaced with our own Approvals /
     AI Access / Activity tabs - no external MCP server management, no
     "Allowed actions" toggles without a backend, no "never ask" option). -->
<script lang="ts">
  import { confirmDialog } from "../../stores/confirm";
  import { onMount } from "svelte";
  import Icon from "../../icons/Icon.svelte";
  import Menu from "../ui/Menu.svelte";
  import { t, format } from "../../i18n";
  import { rpc } from "../../rpc";
  import { hosts, refreshHosts } from "../../stores/items";
  import { aiSessions, pendingApprovals, refreshAiStatus } from "../../stores/ai";
  import { dangerousDialogSession } from "../../stores/ui";
  import { toast, errorText } from "../../stores/toast";
  import { isTelnet, type AuditEntry, type ItemRecord, type Host } from "../../types";

  type Tab = "approvals" | "access" | "activity";
  let tab: Tab = "approvals";
  let busy = false;
  let hostPickerOpen = false;
  let px = 0;
  let py = 0;
  let sessionMenu: { x: number; y: number; session: string } | null = null;
  let audit: AuditEntry[] = [];
  let auditLoading = false;

  $: aiAllowedHosts = $hosts.filter((h) => h.item.ai_allowed);

  async function loadAudit(): Promise<void> {
    auditLoading = true;
    try {
      audit = await rpc.aiAudit({ limit: 50 });
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    } finally {
      auditLoading = false;
    }
  }

  $: if (tab === "activity") void loadAudit();
  onMount(() => void refreshHosts());

  async function decide(approvalId: string, approve: boolean): Promise<void> {
    busy = true;
    try {
      await rpc.aiDecide({ approval_id: approvalId, approve });
      await refreshAiStatus();
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    } finally {
      busy = false;
    }
  }

  async function toggleAiAllowed(h: ItemRecord<Host>, allow: boolean): Promise<void> {
    try {
      await rpc.itemPut<Host>({ kind: "host", id: h.id, item: { ...h.item, ai_allowed: allow } });
      await refreshHosts();
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    }
  }

  function openHostPicker(e: MouseEvent): void {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    px = r.left;
    py = r.bottom + 6;
    hostPickerOpen = true;
  }

  function openSessionMenu(e: MouseEvent, session: string): void {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    sessionMenu = { x: r.right, y: r.bottom + 6, session };
  }

  async function turnOff(aiSession: string): Promise<void> {
    sessionMenu = null;
    busy = true;
    try {
      await rpc.aiDangerous({ ai_session: aiSession, hosts: [], minutes: 0 });
      await refreshAiStatus();
      toast.success($t("toast.dangerousOff"));
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    } finally {
      busy = false;
    }
  }

  async function stopAll(): Promise<void> {
    const ok = await confirmDialog({
      title: $t("ai.stopAll"),
      message: $t("ai.stopAllConfirm"),
      confirmLabel: $t("ai.stopAll"),
    });
    if (!ok) return;
    busy = true;
    try {
      await rpc.aiStop();
      await refreshAiStatus();
      toast.success($t("toast.aiStopped"));
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    } finally {
      busy = false;
    }
  }

  function copy(text: string): void {
    void navigator.clipboard.writeText(text).then(() => toast.success($t("toast.copied")));
  }

  const clientJson = `{\n  "mcpServers": {\n    "ourshell": { "command": "ourshell-mcp" }\n  }\n}`;
</script>

<section class="view">
  <div class="card-head">
    <div class="title-row">
      <h1 class="card-title">{$t("nav.mcp")}</h1>
      {#if $pendingApprovals.length > 0}<span class="badge accent">{$pendingApprovals.length}</span>{/if}
    </div>
    <div class="toolbar">
      <div class="seg">
        <button class:on={tab === "approvals"} on:click={() => (tab = "approvals")}>{$t("mcp.tabApprovals")}</button>
        <button class:on={tab === "access"} on:click={() => (tab = "access")}>{$t("mcp.tabAccess")}</button>
        <button class:on={tab === "activity"} on:click={() => (tab = "activity")}>{$t("mcp.tabActivity")}</button>
      </div>
      <div class="spacer"></div>
      {#if tab === "access"}
        <button class="btn btn-danger btn-sm" disabled={busy} on:click={stopAll}>
          <Icon name="stop" size="sm" /><span>{$t("ai.stopAll")}</span>
        </button>
      {/if}
    </div>
  </div>

  {#if tab === "approvals"}
    <div class="card-body">
      {#if $pendingApprovals.length === 0}
        <div class="empty">
          <Icon name="checkc" size="lg" />
          <strong>{$t("appr.empty")}</strong>
          <p>{$t("appr.emptyHint")}</p>
        </div>
      {:else}
        <div class="grid wide">
          {#each $pendingApprovals as a (a.approval_id)}
            <div class="item" style="flex-direction:column;align-items:stretch;min-height:auto;cursor:default">
              <div class="row">
                <span class="round sm"><Icon name="ai" /></span>
                <span class="meta">
                  <div class="item-title">{a.ai_client_name ?? "AI"}</div>
                  <div class="item-sub">{a.host} · {a.tool}</div>
                </span>
              </div>
              <pre class="code">{a.detail}</pre>
              <div class="row">
                <span class="spacer"></span>
                <button class="btn btn-ghost btn-sm" disabled={busy} on:click={() => decide(a.approval_id, false)}>{$t("appr.reject")}</button>
                <button class="btn btn-primary btn-sm" disabled={busy} on:click={() => decide(a.approval_id, true)}>{$t("appr.approve")}</button>
              </div>
            </div>
          {/each}
        </div>
      {/if}
    </div>
  {:else if tab === "access"}
    <div class="card-body">
      <div class="hero">
        <span class="round"><Icon name="ai" /></span>
        <div class="meta">
          <b>{$t("ai.hero")}</b>
          <p class="hint">{$t("ai.heroBody")}</p>
        </div>
      </div>
      <div class="field">
        <span class="lbl">{$t("ai.claudeCode")}</span>
        <div class="code-wrap">
          <pre class="code">claude mcp add ourshell -- ourshell-mcp</pre>
          <button class="btn btn-soft btn-sm copy-btn" on:click={() => copy("claude mcp add ourshell -- ourshell-mcp")}>
            <Icon name="copy" /><span>{$t("a.copy")}</span>
          </button>
        </div>
      </div>
      <div class="field">
        <span class="lbl">{$t("ai.claudeDesktop")}</span>
        <div class="code-wrap">
          <pre class="code">{clientJson}</pre>
          <button class="btn btn-soft btn-sm copy-btn" on:click={() => copy(clientJson)}>
            <Icon name="copy" /><span>{$t("a.copy")}</span>
          </button>
        </div>
      </div>

      <h2 class="sec-title">{$t("ai.hosts")}</h2>
      <div class="chips">
        {#each aiAllowedHosts as h (h.id)}
          <span class="chip">
            <Icon name="hosts" /><span>{h.item.name}</span>
            <button class="chip-x" aria-label="Remove" on:click={() => toggleAiAllowed(h, false)}><Icon name="x" size="sm" /></button>
          </span>
        {/each}
      </div>
      <button class="add-link" aria-expanded={hostPickerOpen} on:click={openHostPicker}>
        <Icon name="plus" /><span>{$t("ai.allowHost")}</span>
      </button>
      <p class="hint">{$t("ai.hostsHint")}</p>

      <h2 class="sec-title">{$t("ai.lockedRow")}</h2>
      <div class="toggle-row">
        <Icon name="lock" />
        <span class="grow"><b>{$t("ai.lockedRow")}</b><small>{$t("ai.lockedRowSub")}</small></span>
        <label class="switch"><input type="checkbox" checked disabled /><span></span></label>
      </div>

      <h2 class="sec-title">{$t("ai.approvalTitle")}</h2>
      <p class="hint">{$t("ai.approvalBody")}</p>

      <h2 class="sec-title">
        <span>{$t("ai.sessions")}</span>
        <span class="badge">{$aiSessions.length}</span>
      </h2>
      {#if $aiSessions.length === 0}
        <p class="hint">{$t("ai.sessionsEmpty")}</p>
      {:else}
        <div class="grid wide">
          {#each $aiSessions as s (s.ai_session)}
            <div class="item" style="cursor:default">
              <span class="round"><Icon name="ai" /></span>
              <span class="meta">
                <div class="item-title">{s.ai_client_name ?? s.ai_session}</div>
                <div class="item-sub">
                  {#if s.dangerous_until}
                    {format($t("ai.dangerousUntil"), { time: s.dangerous_until })}
                  {:else}
                    {$t("mode.normal")}
                  {/if}
                </div>
              </span>
              {#if s.dangerous_until}<span class="dot err"></span>{/if}
              <span class="item-actions">
                <button class="icon-btn sm" aria-label="More" on:click|stopPropagation={(e) => openSessionMenu(e, s.ai_session)}>
                  <Icon name="dots" />
                </button>
              </span>
            </div>
          {/each}
        </div>
      {/if}
    </div>
  {:else}
    <div class="card-body">
      <p class="hint" style="margin-bottom:14px">{$t("act.limit")}</p>
      {#if auditLoading}
        <p class="hint">Loading…</p>
      {:else if audit.length === 0}
        <div class="empty"><Icon name="history" size="lg" /><strong>{$t("act.empty")}</strong></div>
      {:else}
        <table class="table">
          <thead>
            <tr>
              <th>{$t("t.time")}</th><th>{$t("t.client")}</th><th>{$t("t.action")}</th>
              <th>{$t("t.host")}</th><th>{$t("t.detail")}</th><th>{$t("t.mode")}</th><th>{$t("t.result")}</th>
            </tr>
          </thead>
          <tbody>
            {#each audit as e, i (i)}
              <tr>
                <td class="mono hint">{e.time}</td>
                <td>{e.ai_session}</td>
                <td class="mono">{e.tool}</td>
                <td>{e.host}</td>
                <td class="mono" style="max-width:220px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{e.detail}</td>
                <td>{e.mode === "dangerous" ? $t("mode.dangerous") : e.mode === "auto-readonly" ? $t("mode.autoReadonly") : $t("mode.normal")}</td>
                <td>
                  <span class="badge {e.outcome === 'approved' || e.outcome === 'allowed' ? 'ok' : e.outcome === 'denied' ? 'err' : 'warn'}">
                    {e.outcome}
                  </span>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </div>
  {/if}
</section>

{#if hostPickerOpen}
  <Menu x={px} y={py} onClose={() => (hostPickerOpen = false)}>
    {#each $hosts.filter((h) => !h.item.ai_allowed && !isTelnet(h.item)) as h (h.id)}
      <button class="menu-item" on:click={() => { toggleAiAllowed(h, true); hostPickerOpen = false; }}>
        <Icon name="hosts" /><span>{h.item.name}</span>
      </button>
    {/each}
  </Menu>
{/if}

{#if sessionMenu}
  <Menu x={sessionMenu.x} y={sessionMenu.y} align="right" onClose={() => (sessionMenu = null)}>
    {#if $aiSessions.find((s) => s.ai_session === sessionMenu?.session)?.dangerous_until}
      <button class="menu-item danger" on:click={() => sessionMenu && turnOff(sessionMenu.session)}>
        <Icon name="stop" /><span>{$t("ai.dangerousOff")}</span>
      </button>
    {:else}
      <button
        class="menu-item"
        on:click={() => {
          if (sessionMenu) dangerousDialogSession.set(sessionMenu.session);
          sessionMenu = null;
        }}
      >
        <Icon name="warn" /><span>{$t("ai.dangerousOn")}</span>
      </button>
    {/if}
  </Menu>
{/if}
