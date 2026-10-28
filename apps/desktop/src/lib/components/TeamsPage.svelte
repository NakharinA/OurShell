<script lang="ts">
  import { rpc } from "../rpc";
  import { refreshTeams, teams } from "../stores/teams";
  import { hosts, refreshHosts } from "../stores/items";
  import { showTeams } from "../stores/ui";

  let error = "";
  let busy = false;

  // create form
  let newName = "";
  let newMembers = "";

  // add/remove member form
  let memberTeamId = "";
  let memberEmail = "";

  // share form
  let shareHostId = "";
  let shareTeamId = "";

  $: teamVaults = $teams.filter((t) => t.kind === "team");

  async function load(): Promise<void> {
    busy = true;
    error = "";
    try {
      await refreshTeams();
      await refreshHosts();
    } catch (err) {
      error = (err as Error).message ?? "Could not load teams.";
    } finally {
      busy = false;
    }
  }

  async function create(): Promise<void> {
    if (!newName) {
      error = "Team name is required.";
      return;
    }
    busy = true;
    error = "";
    try {
      const members = newMembers
        .split(",")
        .map((m) => m.trim())
        .filter((m) => m.length > 0);
      await rpc.teamCreate({ name: newName, members });
      await refreshTeams();
      newName = "";
      newMembers = "";
    } catch (err) {
      error = (err as Error).message ?? "Could not create the team.";
    } finally {
      busy = false;
    }
  }

  async function addMember(): Promise<void> {
    if (!memberTeamId || !memberEmail) {
      error = "Choose a team and enter an email.";
      return;
    }
    busy = true;
    error = "";
    try {
      await rpc.teamAddMember({ team: memberTeamId, email: memberEmail });
      memberEmail = "";
    } catch (err) {
      error = (err as Error).message ?? "Could not add the member.";
    } finally {
      busy = false;
    }
  }

  async function removeMember(): Promise<void> {
    if (!memberTeamId || !memberEmail) {
      error = "Choose a team and enter an email.";
      return;
    }
    busy = true;
    error = "";
    try {
      await rpc.teamRemoveMember({ team: memberTeamId, email: memberEmail });
      memberEmail = "";
    } catch (err) {
      error = (err as Error).message ?? "Could not remove the member.";
    } finally {
      busy = false;
    }
  }

  async function share(): Promise<void> {
    if (!shareHostId || !shareTeamId) {
      error = "Choose a host and a team.";
      return;
    }
    busy = true;
    error = "";
    try {
      const record = await rpc.itemGet({ kind: "host", id: shareHostId });
      await rpc.teamShare({ item_id: record.id, team: shareTeamId });
      await refreshHosts();
    } catch (err) {
      error = (err as Error).message ?? "Could not share the host.";
    } finally {
      busy = false;
    }
  }

  function close(): void {
    showTeams.set(false);
    error = "";
  }

  $: if ($showTeams) void load();
</script>

<svelte:window on:keydown={(e) => $showTeams && e.key === "Escape" && close()} />

{#if $showTeams}
  <div class="modal-backdrop" role="presentation" on:click|self={close}>
    <div class="modal wide" role="dialog" aria-modal="true" aria-labelledby="teams-title">
      <h2 id="teams-title">Teams</h2>

      <table>
        <thead>
          <tr><th>Name</th><th>Kind</th><th>Role</th><th>Vault id</th></tr>
        </thead>
        <tbody>
          {#each $teams as t (t.vault_id)}
            <tr>
              <td>{t.name}</td>
              <td>{t.kind}</td>
              <td>{t.role}</td>
              <td class="mono">{t.vault_id}</td>
            </tr>
          {/each}
        </tbody>
      </table>

      <div class="forms">
        <form class="panel col" on:submit|preventDefault={create}>
          <h3>Create a team</h3>
          <label>Name <input bind:value={newName} /></label>
          <label>
            Members (comma-separated emails, optional)
            <input bind:value={newMembers} placeholder="a@example.com, b@example.com" />
          </label>
          <button class="primary" type="submit" disabled={busy}>Create</button>
        </form>

        <form class="panel col" on:submit|preventDefault={addMember}>
          <h3>Add / remove a member</h3>
          <label>
            Team
            <select bind:value={memberTeamId}>
              <option value="">Select a team…</option>
              {#each teamVaults as t (t.vault_id)}
                <option value={t.vault_id}>{t.name}</option>
              {/each}
            </select>
          </label>
          <label>Email <input bind:value={memberEmail} /></label>
          <div class="row">
            <button class="primary" type="submit" disabled={busy}>Add</button>
            <button type="button" class="danger" disabled={busy} on:click={removeMember}>
              Remove (rotates key)
            </button>
          </div>
        </form>

        <form class="panel col" on:submit|preventDefault={share}>
          <h3>Share a host into a team</h3>
          <label>
            Host
            <select bind:value={shareHostId}>
              <option value="">Select a host…</option>
              {#each $hosts as h (h.id)}
                <option value={h.id}>{h.item.name}</option>
              {/each}
            </select>
          </label>
          <label>
            Team
            <select bind:value={shareTeamId}>
              <option value="">Select a team…</option>
              {#each teamVaults as t (t.vault_id)}
                <option value={t.vault_id}>{t.name}</option>
              {/each}
            </select>
          </label>
          <button class="primary" type="submit" disabled={busy}>Share</button>
        </form>
      </div>

      {#if error}<p class="error">{error}</p>{/if}

      <div class="row actions">
        <span class="spacer"></span>
        <button on:click={close}>Close</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .modal.wide {
    width: 820px;
  }
  .mono {
    font-family: var(--mono);
    font-size: 11px;
  }
  .forms {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 12px;
    margin-top: 14px;
  }
  .forms .panel {
    padding: 12px;
  }
  .forms h3 {
    margin: 0 0 6px 0;
    font-size: 13px;
  }
  .error {
    color: var(--danger);
    font-size: 13px;
    margin-top: 10px;
  }
  .actions {
    margin-top: 14px;
  }
</style>
