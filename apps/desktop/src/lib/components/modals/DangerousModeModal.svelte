<!-- Enable dangerous mode for one AI session, limited to AI-allowed hosts,
     for a set number of minutes (default 60). Never a global "never ask". -->
<script lang="ts">
  import Modal from "../ui/Modal.svelte";
  import Icon from "../../icons/Icon.svelte";
  import { t } from "../../i18n";
  import { rpc } from "../../rpc";
  import { hosts } from "../../stores/items";
  import { dangerousDialogSession } from "../../stores/ui";
  import { refreshAiStatus } from "../../stores/ai";
  import { toast, errorText } from "../../stores/toast";

  $: sessionId = $dangerousDialogSession;
  $: aiAllowedHosts = $hosts.filter((h) => h.item.ai_allowed);

  let selected = new Set<string>();
  let minutes = 60;
  let error = "";
  let busy = false;
  let understood = false;

  $: if (sessionId) {
    selected = new Set();
    minutes = 60;
    error = "";
    understood = false;
  }

  function toggle(name: string): void {
    if (selected.has(name)) selected.delete(name);
    else selected.add(name);
    selected = new Set(selected);
  }

  function cancel(): void {
    dangerousDialogSession.set(null);
  }

  async function confirm(): Promise<void> {
    if (!sessionId || selected.size === 0) {
      error = $t("dm.hostsEmpty");
      return;
    }
    busy = true;
    error = "";
    try {
      await rpc.aiDangerous({ ai_session: sessionId, hosts: [...selected], minutes });
      await refreshAiStatus();
      toast.success($t("toast.dangerousOn"));
      cancel();
    } catch (err) {
      error = errorText(err, $t("toast.error"));
    } finally {
      busy = false;
    }
  }
</script>

{#if sessionId}
  <Modal title={$t("dm.title")} onClose={cancel}>
    <div class="psec">
      <div class="alert" style="background:var(--danger-soft);color:var(--danger-text);align-items:flex-start">
        <Icon name="warn" />
        <span>{$t("dm.warning")}</span>
      </div>
      <p class="hint" style="color:var(--danger-text)">{$t("dm.risk")}</p>
    </div>
    <div class="psec">
      <span class="lbl">{$t("dm.hosts")}</span>
      {#if aiAllowedHosts.length === 0}
        <p class="hint">{$t("dm.hostsEmpty")}</p>
      {/if}
      {#each aiAllowedHosts as h (h.id)}
        <label class="toggle-row">
          <input
            type="checkbox"
            checked={selected.has(h.item.name)}
            on:change={() => toggle(h.item.name)}
            style="width:16px;height:16px"
          />
          <span class="grow">{h.item.name}</span>
        </label>
      {/each}
      <div class="field">
        <label for="dm-minutes">{$t("dm.minutes")}</label>
        <div class="input port"><input id="dm-minutes" type="number" min="1" bind:value={minutes} /></div>
      </div>
      <label class="toggle-row">
        <input type="checkbox" bind:checked={understood} style="width:16px;height:16px" />
        <span class="grow">{$t("dm.confirmCheck")}</span>
      </label>
      {#if error}<p class="error-text">{error}</p>{/if}
    </div>
    <svelte:fragment slot="footer">
      <button class="btn btn-ghost" disabled={busy} on:click={cancel}>{$t("a.cancel")}</button>
      <button class="btn btn-danger" disabled={busy || !understood} on:click={confirm}>{$t("dm.submit")}</button>
    </svelte:fragment>
  </Modal>
{/if}
