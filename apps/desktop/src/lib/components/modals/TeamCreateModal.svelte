<script lang="ts">
  import Modal from "../ui/Modal.svelte";
  import { t } from "../../i18n";
  import { rpc } from "../../rpc";
  import { teamCreateOpen } from "../../stores/ui";
  import { refreshTeams } from "../../stores/teams";
  import { toast, errorText } from "../../stores/toast";

  let name = "";
  let membersText = "";
  let busy = false;
  let error = "";

  function close(): void {
    teamCreateOpen.set(false);
    name = "";
    membersText = "";
    error = "";
  }

  async function create(): Promise<void> {
    if (!name.trim()) {
      error = $t("a.required");
      return;
    }
    busy = true;
    error = "";
    try {
      const members = membersText
        .split(",")
        .map((m) => m.trim())
        .filter(Boolean);
      await rpc.teamCreate({ name: name.trim(), members });
      await refreshTeams();
      toast.success($t("toast.teamCreated"));
      close();
    } catch (err) {
      error = errorText(err, $t("toast.error"));
    } finally {
      busy = false;
    }
  }
</script>

{#if $teamCreateOpen}
  <Modal title={$t("vault.createTitle")} onClose={close}>
    <div class="psec">
      <div class="field">
        <label for="team-name">{$t("vault.name")}</label>
        <div class="input"><input id="team-name" bind:value={name} /></div>
      </div>
      <div class="field">
        <label for="team-members">{$t("vault.members")}</label>
        <div class="input"><input id="team-members" bind:value={membersText} placeholder="a@example.com, b@example.com" /></div>
        <p class="hint">{$t("vault.membersHint")}</p>
      </div>
      {#if error}<p class="error-text">{error}</p>{/if}
    </div>
    <svelte:fragment slot="footer">
      <button class="btn btn-ghost" on:click={close}>{$t("a.cancel")}</button>
      <button class="btn btn-primary" disabled={busy} on:click={create}>{$t("a.save")}</button>
    </svelte:fragment>
  </Modal>
{/if}
