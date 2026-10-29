<!-- Port Forwarding (design `data-view="forwarding"`). Dynamic/SOCKS forwards
     and port-forward rules attached to a host aren't supported, so only
     Local/Remote appear here (see ForwardModal). -->
<script lang="ts">
  import Icon from "../../icons/Icon.svelte";
  import Menu from "../ui/Menu.svelte";
  import { t } from "../../i18n";
  import { rpc } from "../../rpc";
  import type { ForwardInfo } from "../../types";
  import { forwardEditorOpen, forwardsVersion } from "../../stores/ui";
  import { toast, errorText } from "../../stores/toast";

  let forwards: ForwardInfo[] = [];
  let loading = false;
  let menu: { x: number; y: number; fwd: ForwardInfo } | null = null;

  async function refresh(): Promise<void> {
    loading = true;
    try {
      forwards = await rpc.forwardList();
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    } finally {
      loading = false;
    }
  }

  $: void $forwardsVersion, refresh();

  function openMenu(e: MouseEvent, fwd: ForwardInfo): void {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    menu = { x: r.right, y: r.bottom + 6, fwd };
  }

  async function stop(fwd: ForwardInfo): Promise<void> {
    menu = null;
    try {
      await rpc.forwardClose(fwd.id);
      toast.success($t("toast.forwardClosed"));
      await refresh();
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    }
  }
</script>

<section class="view">
  <div class="card-head">
    <div class="title-row">
      <h1 class="card-title">{$t("fwd.title")}</h1>
      <span class="badge accent">{forwards.length}</span>
    </div>
    <div class="toolbar">
      <button class="btn btn-primary" on:click={() => forwardEditorOpen.set(true)}>
        <Icon name="plus" /><span>{$t("fwd.new")}</span>
      </button>
      <div class="spacer"></div>
    </div>
  </div>
  <div class="card-body">
    {#if !loading && forwards.length === 0}
      <div class="empty">
        <Icon name="forward" size="lg" />
        <strong>{$t("fwd.empty")}</strong>
        <p>{$t("fwd.emptyHint")}</p>
      </div>
    {:else}
      <div class="grid wide">
        {#each forwards as f (f.id)}
          <div class="item" role="button" tabindex="0">
            <span class="round letter">{f.kind === "local" ? "L" : "R"}</span>
            <span class="meta">
              <div class="item-title">{f.kind === "local" ? $t("f.local") : $t("f.remote")}</div>
              <div class="item-sub mono">{f.bind} → {f.dest} via {f.host}</div>
            </span>
            <span class="item-actions">
              <button class="icon-btn sm" aria-label="More" on:click|stopPropagation={(e) => openMenu(e, f)}>
                <Icon name="dots" />
              </button>
            </span>
          </div>
        {/each}
      </div>
      <p class="hint" style="margin-top:18px">{$t("fwd.hint")}</p>
    {/if}
  </div>
</section>

{#if menu}
  <Menu x={menu.x} y={menu.y} align="right" onClose={() => (menu = null)}>
    <button class="menu-item danger" on:click={() => menu && stop(menu.fwd)}>
      <Icon name="x" /><span>{$t("fwd.stop")}</span>
    </button>
  </Menu>
{/if}
