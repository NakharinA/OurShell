<script lang="ts">
  import { rpc } from "../rpc";
  import type { ItemRecord, KeyAlgorithm, SshKey } from "../types";
  import { keys, refreshKeys } from "../stores/items";
  import { showKeys } from "../stores/ui";

  let mode: "generate" | "import" = "generate";
  let error = "";
  let busy = false;

  // generate form
  let genName = "";
  let genAlgorithm: KeyAlgorithm = "ed25519";
  let genBits = 4096;
  let genPassphrase = "";

  // import form
  let impName = "";
  let impPrivateKey = "";
  let impPassphrase = "";

  async function generate(): Promise<void> {
    if (!genName) {
      error = "Name is required.";
      return;
    }
    busy = true;
    error = "";
    try {
      await rpc.keyGenerate({
        name: genName,
        algorithm: genAlgorithm,
        bits: genAlgorithm === "rsa" ? genBits : null,
        passphrase: genPassphrase || null,
      });
      await refreshKeys();
      genName = "";
      genPassphrase = "";
    } catch (err) {
      error = (err as Error).message ?? "Could not generate the key.";
    } finally {
      busy = false;
    }
  }

  async function importKey(): Promise<void> {
    if (!impName || !impPrivateKey) {
      error = "Name and private key are required.";
      return;
    }
    busy = true;
    error = "";
    try {
      await rpc.keyImport({
        name: impName,
        private_key: impPrivateKey,
        passphrase: impPassphrase || null,
      });
      await refreshKeys();
      impName = "";
      impPrivateKey = "";
      impPassphrase = "";
    } catch (err) {
      error = (err as Error).message ?? "Could not import the key.";
    } finally {
      busy = false;
    }
  }

  async function remove(rec: ItemRecord<SshKey>): Promise<void> {
    busy = true;
    try {
      await rpc.itemDelete({ kind: "key", id: rec.id });
      await refreshKeys();
    } finally {
      busy = false;
    }
  }

  function close(): void {
    showKeys.set(false);
    error = "";
  }
</script>

<svelte:window on:keydown={(e) => $showKeys && e.key === "Escape" && close()} />

{#if $showKeys}
  <div class="modal-backdrop" role="presentation" on:click|self={close}>
    <div class="modal wide" role="dialog" aria-modal="true" aria-labelledby="keys-title">
      <h2 id="keys-title">Identities & keys</h2>
      <div class="layout">
        <table>
          <thead>
            <tr><th>Name</th><th>Public key</th><th></th></tr>
          </thead>
          <tbody>
            {#each $keys as rec (rec.id)}
              <tr>
                <td>{rec.item.name}</td>
                <td class="pubkey">{rec.item.public_key}</td>
                <td><button class="ghost" on:click={() => remove(rec)}>Delete</button></td>
              </tr>
            {/each}
          </tbody>
        </table>
        <div class="panel form-card">
          <div class="row tabs">
            <button class:active={mode === "generate"} on:click={() => (mode = "generate")}>Generate</button>
            <button class:active={mode === "import"} on:click={() => (mode = "import")}>Import</button>
          </div>
          {#if mode === "generate"}
            <form on:submit|preventDefault={generate} class="col">
              <label>Name <input bind:value={genName} /></label>
              <label>
                Algorithm
                <select bind:value={genAlgorithm}>
                  <option value="ed25519">ed25519</option>
                  <option value="rsa">RSA</option>
                </select>
              </label>
              {#if genAlgorithm === "rsa"}
                <label>Bits <input type="number" bind:value={genBits} /></label>
              {/if}
              <label>Passphrase (optional) <input type="password" bind:value={genPassphrase} /></label>
              <button class="primary" type="submit" disabled={busy}>Generate</button>
            </form>
          {:else}
            <form on:submit|preventDefault={importKey} class="col">
              <label>Name <input bind:value={impName} /></label>
              <label>Private key <textarea rows="6" bind:value={impPrivateKey}></textarea></label>
              <label>Passphrase (optional) <input type="password" bind:value={impPassphrase} /></label>
              <button class="primary" type="submit" disabled={busy}>Import</button>
            </form>
          {/if}
          {#if error}<p class="error">{error}</p>{/if}
        </div>
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
    width: 760px;
  }
  .layout {
    display: grid;
    grid-template-columns: 1fr 300px;
    gap: 16px;
    align-items: start;
  }
  .pubkey {
    font-family: var(--mono);
    font-size: 11px;
    max-width: 320px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .form-card {
    padding: 14px;
  }
  .tabs button.active {
    border-color: var(--accent);
    color: var(--accent);
  }
  .error {
    color: var(--danger);
    font-size: 13px;
  }
  .actions {
    margin-top: 14px;
  }
</style>
