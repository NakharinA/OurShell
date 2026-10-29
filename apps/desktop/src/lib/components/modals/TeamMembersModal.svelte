<script lang="ts">
  import Modal from "../ui/Modal.svelte";
  import Icon from "../../icons/Icon.svelte";
  import { t } from "../../i18n";
  import { rpc } from "../../rpc";
  import { teamMembersOpen } from "../../stores/ui";
  import { teams } from "../../stores/teams";
  import { toast, errorText } from "../../stores/toast";

  $: vaultId = $teamMembersOpen;
  $: team = vaultId ? $teams.find((t2) => t2.vault_id === vaultId) : null;

  let email = "";
  let busy = false;
  let error = "";

  function close(): void {
    teamMembersOpen.set(null);
    email = "";
    error = "";
  }

  async function add(): Promise<void> {
    if (!vaultId || !email.trim()) return;
    busy = true;
    error = "";
    try {
      await rpc.teamAddMember({ team: vaultId, email: email.trim() });
      toast.success($t("toast.memberAdded"));
      email = "";
    } catch (err) {
      error = errorText(err, $t("toast.error"));
    } finally {
      busy = false;
    }
  }

  async function remove(memberEmail: string): Promise<void> {
    if (!vaultId) return;
    if (!confirm(`${$t("vault.removeMember")} ${memberEmail}? ${$t("vault.removeMemberHint")}`)) return;
    busy = true;
    error = "";
    try {
      await rpc.teamRemoveMember({ team: vaultId, email: memberEmail });
      toast.success($t("toast.memberRemoved"));
    } catch (err) {
      error = errorText(err, $t("toast.error"));
    } finally {
      busy = false;
    }
  }
</script>

{#if vaultId}
  <Modal title={team ? `${$t("vault.manageMembers")} — ${team.name}` : $t("vault.manageMembers")} onClose={close}>
    <div class="psec">
      <p class="hint">
        <Icon name="users" size="sm" />
        {$t("vault.removeMemberHint")}
      </p>
      <div class="row">
        <div class="input grow"><input bind:value={email} placeholder="name@example.com" /></div>
        <button class="btn btn-soft" disabled={busy || !email.trim()} on:click={() => remove(email.trim())}>
          {$t("vault.removeMember")}
        </button>
        <button class="btn btn-primary" disabled={busy || !email.trim()} on:click={add}>{$t("vault.addMember")}</button>
      </div>
      {#if error}<p class="error-text">{error}</p>{/if}
    </div>
    <svelte:fragment slot="footer">
      <button class="btn btn-ghost" on:click={close}>{$t("p.close")}</button>
    </svelte:fragment>
  </Modal>
{/if}
