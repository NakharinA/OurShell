<script lang="ts">
  import { rpc } from "../rpc";
  import { emptyHost, type Host } from "../types";
  import { groups, identities, hosts, refreshHosts } from "../stores/items";
  import { hostEditor } from "../stores/ui";

  $: state = $hostEditor;
  $: record = state?.mode === "edit" ? state.record : null;
  let form: Host = emptyHost();
  let tagsText = "";
  let error = "";
  let busy = false;
  let lastId: string | null | undefined;

  $: initFromState(state);

  function initFromState(s: typeof state): void {
    if (!s) return;
    const key = s.mode === "edit" ? s.record.id : "new";
    if (key === lastId) return;
    lastId = key;
    if (s.mode === "edit") {
      form = { ...s.record.item };
    } else {
      form = emptyHost();
      if (s.groupId) form.group_id = s.groupId;
    }
    tagsText = form.tags.join(", ");
    error = "";
  }

  function close(): void {
    hostEditor.set(null);
    lastId = null;
  }

  async function save(): Promise<void> {
    if (!form.name || !form.hostname) {
      error = "Name and hostname are required.";
      return;
    }
    busy = true;
    error = "";
    try {
      form.tags = tagsText
        .split(",")
        .map((t) => t.trim())
        .filter(Boolean);
      await rpc.itemPut<Host>({
        kind: "host",
        id: record?.id ?? null,
        item: form,
      });
      await refreshHosts();
      close();
    } catch (err) {
      error = (err as Error).message ?? "Could not save the host.";
    } finally {
      busy = false;
    }
  }

  async function remove(): Promise<void> {
    if (!record) return;
    busy = true;
    try {
      await rpc.itemDelete({ kind: "host", id: record.id });
      await refreshHosts();
      close();
    } catch (err) {
      error = (err as Error).message ?? "Could not delete the host.";
    } finally {
      busy = false;
    }
  }
</script>

{#if state}
  <div class="modal-backdrop" on:click={close}>
    <div class="modal" on:click|stopPropagation>
      <h2>{record ? "Edit host" : "New host"}</h2>
      <div class="grid">
        <label>
          Name
          <input bind:value={form.name} placeholder="prod-web" />
        </label>
        <label>
          Hostname
          <input bind:value={form.hostname} placeholder="10.0.0.5 or example.com" />
        </label>
        <label>
          Port
          <input type="number" bind:value={form.port} min="1" max="65535" />
        </label>
        <label>
          Username
          <input bind:value={form.username} placeholder="(optional)" />
        </label>
        <label>
          Identity
          <select bind:value={form.identity_id}>
            <option value={null}>(none)</option>
            {#each $identities as id (id.id)}
              <option value={id.id}>{id.item.name}</option>
            {/each}
          </select>
        </label>
        <label>
          Group
          <select bind:value={form.group_id}>
            <option value={null}>(ungrouped)</option>
            {#each $groups as g (g.id)}
              <option value={g.id}>{g.item.name}</option>
            {/each}
          </select>
        </label>
        <label>
          Jump host
          <select bind:value={form.jump_host}>
            <option value={null}>(none)</option>
            {#each $hosts.filter((h) => h.id !== record?.id) as h (h.id)}
              <option value={h.item.name}>{h.item.name}</option>
            {/each}
          </select>
        </label>
        <label>
          Tags (comma separated)
          <input bind:value={tagsText} placeholder="prod, web" />
        </label>
      </div>
      <label class="checkbox">
        <input type="checkbox" bind:checked={form.ai_allowed} />
        Allow AI tools to use this host
      </label>
      {#if form.ai_allowed}
        <label>
          AI user override
          <input bind:value={form.ai_user} placeholder="e.g. a restricted `ai` user" />
        </label>
      {/if}
      <label>
        Notes
        <textarea rows="3" bind:value={form.notes}></textarea>
      </label>
      {#if error}
        <p class="error">{error}</p>
      {/if}
      <div class="row actions">
        {#if record}
          <button class="danger" on:click={remove} disabled={busy}>Delete</button>
        {/if}
        <span class="spacer"></span>
        <button on:click={close} disabled={busy}>Cancel</button>
        <button class="primary" on:click={save} disabled={busy}>Save</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .modal {
    width: 480px;
  }
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
    margin-bottom: 10px;
  }
  .checkbox {
    flex-direction: row;
    align-items: center;
    gap: 8px;
    margin-bottom: 8px;
  }
  .error {
    color: var(--danger);
    font-size: 13px;
  }
  .actions {
    margin-top: 14px;
  }
</style>
