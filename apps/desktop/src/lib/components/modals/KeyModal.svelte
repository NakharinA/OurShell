<!-- Generate / import a key (design `data-modal="keychain"`). -->
<script lang="ts">
  import { open as openDialog } from "@tauri-apps/plugin-dialog";
  import { readFile } from "@tauri-apps/plugin-fs";
  import Modal from "../ui/Modal.svelte";
  import Icon from "../../icons/Icon.svelte";
  import { t } from "../../i18n";
  import { rpc } from "../../rpc";
  import type { KeyAlgorithm } from "../../types";
  import { refreshKeys } from "../../stores/items";
  import { keyEditor, keyPickerReturn } from "../../stores/ui";
  import { toast, errorText } from "../../stores/toast";

  $: state = $keyEditor;
  let tab: "generate" | "import" = "generate";
  let lastOpen = false;

  $: if (state && !lastOpen) {
    tab = state.kind ?? "generate";
    reset();
  }
  $: lastOpen = !!state;

  // generate form
  let genName = "";
  let genAlgorithm: KeyAlgorithm = "ed25519";
  let genBits = 4096;
  let genPassphrase = "";

  // import form
  let impName = "";
  let impPrivateKey = "";
  let impPassphrase = "";
  let dragOver = false;

  let busy = false;
  let error = "";
  let resultKey: { name: string; public_key: string } | null = null;

  function reset(): void {
    genName = "";
    genPassphrase = "";
    impName = "";
    impPrivateKey = "";
    impPassphrase = "";
    error = "";
    resultKey = null;
  }

  function close(): void {
    keyEditor.set(null);
    keyPickerReturn.set(null);
  }

  function finish(id: string, name: string, publicKey: string): void {
    const cb = $keyPickerReturn;
    if (cb) {
      cb(id);
      keyPickerReturn.set(null);
      close();
      toast.success($t("toast.keySaved"));
    } else {
      resultKey = { name, public_key: publicKey };
    }
  }

  async function generate(): Promise<void> {
    if (!genName) {
      error = $t("a.required");
      return;
    }
    busy = true;
    error = "";
    try {
      const res = await rpc.keyGenerate({
        name: genName,
        algorithm: genAlgorithm,
        bits: genAlgorithm === "rsa" ? genBits : null,
        passphrase: genPassphrase || null,
      });
      await refreshKeys();
      finish(res.id, genName, res.public_key);
    } catch (err) {
      error = errorText(err, $t("toast.error"));
    } finally {
      busy = false;
    }
  }

  async function doImport(): Promise<void> {
    if (!impName || !impPrivateKey) {
      error = $t("a.required");
      return;
    }
    busy = true;
    error = "";
    try {
      const res = await rpc.keyImport({
        name: impName,
        private_key: impPrivateKey,
        passphrase: impPassphrase || null,
      });
      await refreshKeys();
      finish(res.id, impName, res.public_key);
    } catch (err) {
      error = errorText(err, $t("toast.error"));
    } finally {
      busy = false;
    }
  }

  async function pickFile(): Promise<void> {
    const path = await openDialog({ multiple: false });
    if (!path || Array.isArray(path)) return;
    const bytes = await readFile(path);
    impPrivateKey = new TextDecoder().decode(bytes);
    if (!impName) {
      const base = path.split(/[\\/]/).pop() ?? "key";
      impName = base.replace(/\.[^.]+$/, "");
    }
  }

  function copyPublicKey(): void {
    if (!resultKey) return;
    void navigator.clipboard.writeText(resultKey.public_key).then(() => toast.success($t("keys.pubCopied")));
  }
</script>

{#if state}
  <Modal title={resultKey ? resultKey.name : $t("keys.panel")} onClose={close}>
    <svelte:fragment slot="footer">
      {#if resultKey}
        <button class="btn btn-primary" on:click={close}>{$t("p.close")}</button>
      {:else}
        <button class="btn btn-ghost" on:click={close}>{$t("a.cancel")}</button>
        {#if tab === "generate"}
          <button class="btn btn-primary" disabled={busy} on:click={generate}>{$t("a.save")}</button>
        {:else}
          <button class="btn btn-primary" disabled={busy} on:click={doImport}>{$t("a.save")}</button>
        {/if}
      {/if}
    </svelte:fragment>
    {#if resultKey}
      <div class="psec">
        <div class="field">
          <label for="pub-out">{$t("k.public")}</label>
          <div class="code-wrap">
            <pre class="code" id="pub-out">{resultKey.public_key}</pre>
            <button class="btn btn-soft btn-sm copy-btn" on:click={copyPublicKey}>
              <Icon name="copy" /><span>{$t("a.copy")}</span>
            </button>
          </div>
        </div>
        <p class="hint">{$t("keys.pubHint")}</p>
      </div>
    {:else}
      <div class="psec">
        <div class="seg">
          <button class:on={tab === "generate"} on:click={() => (tab = "generate")}>{$t("keys.generate")}</button>
          <button class:on={tab === "import"} on:click={() => (tab = "import")}>{$t("keys.importFile")}</button>
        </div>
      </div>
      {#if tab === "generate"}
        <div class="psec">
          <div class="field">
            <label for="gen-name">{$t("h.label")}</label>
            <div class="input"><input id="gen-name" bind:value={genName} placeholder={$t("k.labelPh")} /></div>
          </div>
          <div class="field">
            <span class="lbl">{$t("k.algorithm")}</span>
            <div class="seg">
              <button class:on={genAlgorithm === "ed25519"} on:click={() => (genAlgorithm = "ed25519")}>ed25519</button>
              <button class:on={genAlgorithm === "rsa"} on:click={() => (genAlgorithm = "rsa")}>RSA</button>
            </div>
          </div>
          {#if genAlgorithm === "rsa"}
            <div class="field">
              <label for="gen-bits">{$t("k.bits")}</label>
              <div class="input"><input id="gen-bits" type="number" bind:value={genBits} /></div>
            </div>
          {/if}
          <div class="field">
            <label for="gen-pass">{$t("k.passphrase")}</label>
            <div class="input"><input id="gen-pass" type="password" bind:value={genPassphrase} /></div>
          </div>
        </div>
      {:else}
        <div class="psec">
          <div
            class="dropzone"
            class:over={dragOver}
            role="button"
            tabindex="0"
            on:click={pickFile}
            on:keydown={(e) => e.key === "Enter" && pickFile()}
          >
            <Icon name="import" />
            <span>{$t("k.drop")}</span>
          </div>
          <button class="btn btn-primary" on:click={pickFile}>
            <Icon name="import" /><span>{$t("k.import")}</span>
          </button>
          <div class="field">
            <label for="imp-name">{$t("h.label")}</label>
            <div class="input"><input id="imp-name" bind:value={impName} placeholder={$t("k.labelPh")} /></div>
          </div>
          <div class="field">
            <label for="imp-key">{$t("k.private")} ({$t("k.orPaste")})</label>
            <div class="textarea"><textarea id="imp-key" class="mono" bind:value={impPrivateKey} placeholder="-----BEGIN OPENSSH PRIVATE KEY-----"></textarea></div>
          </div>
          <div class="field">
            <label for="imp-pass">{$t("k.passphrase")}</label>
            <div class="input"><input id="imp-pass" type="password" bind:value={impPassphrase} /></div>
          </div>
        </div>
      {/if}
      {#if error}<p class="error-text" style="padding:0 24px">{error}</p>{/if}
    {/if}
  </Modal>
{/if}
