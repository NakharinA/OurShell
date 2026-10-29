<!-- "Share to team..." from a host's context menu. -->
<script lang="ts">
  import Modal from "../ui/Modal.svelte";
  import Icon from "../../icons/Icon.svelte";
  import { t } from "../../i18n";
  import { rpc } from "../../rpc";
  import { shareHostModal } from "../../stores/ui";
  import { refreshHosts } from "../../stores/items";
  import { teams } from "../../stores/teams";
  import { toast, errorText } from "../../stores/toast";

  $: host = $shareHostModal;
  $: teamVaults = $teams.filter((v) => v.kind === "team");
  let busy = false;

  function close(): void {
    shareHostModal.set(null);
  }

  async function share(teamId: string): Promise<void> {
    if (!host) return;
    busy = true;
    try {
      await rpc.teamShare({ item_id: host.id, team: teamId });
      await refreshHosts();
      toast.success($t("toast.shared"));
      close();
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    } finally {
      busy = false;
    }
  }
</script>

{#if host}
  <Modal title={$t("c.shareTeam")} onClose={close}>
    <div class="psec">
      {#if teamVaults.length === 0}
        <p class="hint">{$t("vault.noTeams")}</p>
      {:else}
        <div class="grid">
          {#each teamVaults as v (v.vault_id)}
            <button class="item" style="text-align:left" disabled={busy} on:click={() => share(v.vault_id)}>
              <span class="round"><Icon name="users" /></span>
              <span class="meta"><div class="item-title">{v.name}</div></span>
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
