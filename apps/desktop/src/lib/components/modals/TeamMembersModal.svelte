<script lang="ts">
  import { confirmDialog } from "../../stores/confirm";
  import Modal from "../ui/Modal.svelte";
  import Icon from "../../icons/Icon.svelte";
  import { t } from "../../i18n";
  import { rpc } from "../../rpc";
  import type { TeamMember } from "../../types";
  import { teamMembersOpen } from "../../stores/ui";
  import { teams } from "../../stores/teams";
  import { toast, errorText } from "../../stores/toast";

  $: vaultId = $teamMembersOpen;
  $: team = vaultId ? $teams.find((t2) => t2.vault_id === vaultId) : null;
  $: if (vaultId) void load(vaultId);

  let members: TeamMember[] = [];
  let loading = false;
  let email = "";
  let busy = false;
  let error = "";

  async function load(id: string): Promise<void> {
    loading = true;
    error = "";
    try {
      members = await rpc.teamMembers(id);
    } catch (err) {
      members = [];
      error = errorText(err, $t("toast.error"));
    } finally {
      loading = false;
    }
  }

  function close(): void {
    teamMembersOpen.set(null);
    members = [];
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
      await load(vaultId);
    } catch (err) {
      error = errorText(err, $t("toast.error"));
    } finally {
      busy = false;
    }
  }

  async function remove(memberEmail: string): Promise<void> {
    if (!vaultId) return;
    const ok = await confirmDialog({
      title: `${$t("vault.removeMember")} ${memberEmail}?`,
      message: $t("vault.removeMemberHint"),
      confirmLabel: $t("vault.removeMember"),
    });
    if (!ok) return;
    busy = true;
    error = "";
    try {
      await rpc.teamRemoveMember({ team: vaultId, email: memberEmail });
      toast.success($t("toast.memberRemoved"));
      await load(vaultId);
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
      <h3><Icon name="users" /><span>{$t("vault.memberList")}</span><span class="spacer"></span><span class="badge">{members.length}</span></h3>
      {#if loading}
        <p class="hint">…</p>
      {:else if members.length === 0}
        <p class="hint">{$t("vault.noMembers")}</p>
      {:else}
        {#each members as m (m.user_id)}
          <div class="tool">
            <span class="name">{m.email}</span>
            <button class="btn btn-ghost btn-sm" disabled={busy} on:click={() => remove(m.email)}>
              {$t("vault.removeMember")}
            </button>
          </div>
        {/each}
      {/if}
      <p class="hint">{$t("vault.removeMemberHint")}</p>
    </div>
    <div class="psec">
      <div class="row">
        <div class="input grow"><input bind:value={email} placeholder={$t("vault.addMemberPh")} /></div>
        <button class="btn btn-primary" disabled={busy || !email.trim()} on:click={add}>{$t("vault.addMember")}</button>
      </div>
      {#if error}<p class="error-text">{error}</p>{/if}
    </div>
    <svelte:fragment slot="footer">
      <button class="btn btn-ghost" on:click={close}>{$t("p.close")}</button>
    </svelte:fragment>
  </Modal>
{/if}
