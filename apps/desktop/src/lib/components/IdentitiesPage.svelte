<script lang="ts">
  import { rpc } from "../rpc";
  import type { Identity, ItemRecord } from "../types";
  import { identities, keys, refreshIdentities } from "../stores/items";
  import { showIdentities } from "../stores/ui";

  let editing: ItemRecord<Identity> | null = null;
  let form: Identity = { name: "", username: null, password: null, key_id: null };
  let error = "";
  let busy = false;

  function reset(): void {
    editing = null;
    form = { name: "", username: null, password: null, key_id: null };
    error = "";
  }

  function edit(rec: ItemRecord<Identity>): void {
    editing = rec;
    form = { ...rec.item };
  }

  async function save(): Promise<void> {
    if (!form.name) {
      error = "Name is required.";
      return;
    }
    busy = true;
    error = "";
    try {
      await rpc.itemPut<Identity>({ kind: "identity", id: editing?.id ?? null, item: form });
      await refreshIdentities();
      reset();
    } catch (err) {
      error = (err as Error).message ?? "Could not save the identity.";
    } finally {
      busy = false;
    }
  }

  async function remove(rec: ItemRecord<Identity>): Promise<void> {
    busy = true;
    try {
      await rpc.itemDelete({ kind: "identity", id: rec.id });
      await refreshIdentities();
      if (editing?.id === rec.id) reset();
    } finally {
      busy = false;
    }
  }

  function close(): void {
    showIdentities.set(false);
    reset();
  }
</script>

{#if $showIdentities}
  <div class="modal-backdrop" on:click={close}>
    <div class="modal wide" on:click|stopPropagation>
      <h2>Identities</h2>
      <div class="layout">
        <table>
          <thead>
            <tr><th>Name</th><th>Username</th><th>Key</th><th></th></tr>
          </thead>
          <tbody>
            {#each $identities as rec (rec.id)}
              <tr>
                <td>{rec.item.name}</td>
                <td>{rec.item.username ?? ""}</td>
                <td>{$keys.find((k) => k.id === rec.item.key_id)?.item.name ?? ""}</td>
                <td class="row">
                  <button class="ghost" on:click={() => edit(rec)}>Edit</button>
                  <button class="ghost" on:click={() => remove(rec)}>Delete</button>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
        <form class="panel form-card" on:submit|preventDefault={save}>
          <h3>{editing ? "Edit identity" : "New identity"}</h3>
          <label>Name <input bind:value={form.name} /></label>
          <label>Username <input bind:value={form.username} /></label>
          <label>Password <input type="password" bind:value={form.password} /></label>
          <label>
            Key
            <select bind:value={form.key_id}>
              <option value={null}>(none)</option>
              {#each $keys as k (k.id)}
                <option value={k.id}>{k.item.name}</option>
              {/each}
            </select>
          </label>
          {#if error}<p class="error">{error}</p>{/if}
          <div class="row">
            {#if editing}<button type="button" on:click={reset}>New</button>{/if}
            <span class="spacer"></span>
            <button class="primary" type="submit" disabled={busy}>Save</button>
          </div>
        </form>
      </div>
      <div class="row actions">
        <span class="spacer"></span>
        <button on:click={close}>Close</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .modal.wide {
    width: 720px;
  }
  .layout {
    display: grid;
    grid-template-columns: 1fr 260px;
    gap: 16px;
    align-items: start;
  }
  .form-card {
    padding: 14px;
    gap: 10px;
    display: flex;
    flex-direction: column;
  }
  .error {
    color: var(--danger);
    font-size: 13px;
  }
  .actions {
    margin-top: 14px;
  }
</style>
