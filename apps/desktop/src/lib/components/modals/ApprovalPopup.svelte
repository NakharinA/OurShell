<!-- Pops up (plus an OS notification, see stores/ai.ts) whenever a new AI
     approval request arrives, from any tab - Vaults, terminal, or SFTP. -->
<script lang="ts">
  import Modal from "../ui/Modal.svelte";
  import Icon from "../../icons/Icon.svelte";
  import { t } from "../../i18n";
  import { pendingApprovals, refreshAiStatus } from "../../stores/ai";
  import { rpc } from "../../rpc";
  import { toast, errorText } from "../../stores/toast";
  import type { ApprovalRequest } from "../../types";

  let seen = new Set<string>();
  let shown: ApprovalRequest | null = null;
  let busy = false;

  $: onApprovals($pendingApprovals);
  function onApprovals(list: ApprovalRequest[]): void {
    if (shown && !list.some((a) => a.approval_id === shown?.approval_id)) {
      shown = null;
    }
    if (!shown) {
      const next = list.find((a) => !seen.has(a.approval_id));
      if (next) {
        seen.add(next.approval_id);
        shown = next;
      }
    }
  }

  async function decide(approve: boolean): Promise<void> {
    if (!shown) return;
    busy = true;
    try {
      await rpc.aiDecide({ approval_id: shown.approval_id, approve });
      toast.success(approve ? $t("toast.approved") : $t("toast.rejected"));
      await refreshAiStatus();
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    } finally {
      busy = false;
      shown = null;
    }
  }
</script>

{#if shown}
  <Modal title={$t("appr.modalTitle")} onClose={() => (shown = null)}>
    <div class="psec">
      <div class="row">
        <Icon name="ai" />
        <b>{shown.ai_client_name ?? "AI"}</b>
      </div>
      <div class="row"><span class="lbl">{$t("t.host")}</span><span>{shown.host}</span></div>
      <div class="row"><span class="lbl">{$t("t.action")}</span><span class="mono">{shown.tool}</span></div>
      <pre class="code">{shown.detail}</pre>
    </div>
    <svelte:fragment slot="footer">
      <button class="btn btn-ghost" disabled={busy} on:click={() => decide(false)}>{$t("appr.reject")}</button>
      <button class="btn btn-primary" disabled={busy} on:click={() => decide(true)}>{$t("appr.approve")}</button>
    </svelte:fragment>
  </Modal>
{/if}
