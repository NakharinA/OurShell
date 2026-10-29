<!-- Logs (design `data-view="logs"`) - full AI audit log, no plan upsell. -->
<script lang="ts">
  import { onMount } from "svelte";
  import Icon from "../../icons/Icon.svelte";
  import { t } from "../../i18n";
  import { rpc } from "../../rpc";
  import { toast, errorText } from "../../stores/toast";
  import type { AuditEntry } from "../../types";

  let entries: AuditEntry[] = [];
  let loading = false;
  let search = "";
  let hostFilter = "";
  let outcomeFilter = "";

  async function load(): Promise<void> {
    loading = true;
    try {
      entries = await rpc.aiAudit({ limit: 500 });
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    } finally {
      loading = false;
    }
  }
  onMount(load);

  $: hostOptions = [...new Set(entries.map((e) => e.host))].filter(Boolean).sort();
  $: outcomeOptions = [...new Set(entries.map((e) => e.outcome))].filter(Boolean).sort();
  $: filtered = entries.filter((e) => {
    if (hostFilter && e.host !== hostFilter) return false;
    if (outcomeFilter && e.outcome !== outcomeFilter) return false;
    const q = search.trim().toLowerCase();
    if (!q) return true;
    return (
      e.host.toLowerCase().includes(q) ||
      e.tool.toLowerCase().includes(q) ||
      e.detail.toLowerCase().includes(q) ||
      e.outcome.toLowerCase().includes(q)
    );
  });
</script>

<section class="view">
  <div class="card-head">
    <div class="title-row">
      <h1 class="card-title">{$t("logs.title")}</h1>
      <span class="spacer"></span>
      <label class="search" style="max-width:320px">
        <Icon name="search" />
        <input bind:value={search} placeholder={$t("logs.search")} />
      </label>
    </div>
    <div class="toolbar">
      <select class="input" style="width:auto;min-height:36px" bind:value={hostFilter}>
        <option value="">{$t("logs.allHosts")}</option>
        {#each hostOptions as h (h)}<option value={h}>{h}</option>{/each}
      </select>
      <select class="input" style="width:auto;min-height:36px" bind:value={outcomeFilter}>
        <option value="">{$t("logs.allOutcomes")}</option>
        {#each outcomeOptions as o (o)}<option value={o}>{o}</option>{/each}
      </select>
      <div class="spacer"></div>
    </div>
  </div>
  <div class="card-body">
    {#if loading}
      <p class="hint">Loading…</p>
    {:else if filtered.length === 0}
      <div class="empty"><Icon name="history" size="lg" /><strong>{$t("logs.empty")}</strong></div>
    {:else}
      <table class="table">
        <thead>
          <tr>
            <th>{$t("t.time")}</th><th>{$t("t.client")}</th><th>{$t("t.action")}</th>
            <th>{$t("t.host")}</th><th>{$t("t.detail")}</th><th>{$t("t.mode")}</th><th>{$t("t.result")}</th>
          </tr>
        </thead>
        <tbody>
          {#each filtered as e, i (i)}
            <tr>
              <td class="mono hint">{e.time}</td>
              <td>{e.ai_session}</td>
              <td class="mono">{e.tool}</td>
              <td>{e.host}</td>
              <td class="mono" style="max-width:260px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{e.detail}</td>
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
</section>
