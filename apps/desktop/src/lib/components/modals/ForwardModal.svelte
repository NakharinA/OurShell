<!-- New port forward (design `data-modal="forwarding"`). Local/Remote only -
     Dynamic (SOCKS) forwarding is hidden, it's not supported. -->
<script lang="ts">
  import Modal from "../ui/Modal.svelte";
  import Icon from "../../icons/Icon.svelte";
  import Menu from "../ui/Menu.svelte";
  import { t } from "../../i18n";
  import { rpc } from "../../rpc";
  import type { ForwardKind } from "../../types";
  import { hosts } from "../../stores/items";
  import { forwardEditorOpen, forwardsVersion } from "../../stores/ui";
  import { toast, errorText } from "../../stores/toast";

  let kind: ForwardKind = "local";
  let label = "";
  let localPort = "";
  let bind = "127.0.0.1";
  let viaHost = "";
  let destAddr = "";
  let destPort = "";
  let remotePort = "";
  let busy = false;
  let error = "";
  let hostPickerOpen = false;
  let px = 0;
  let py = 0;

  function close(): void {
    forwardEditorOpen.set(false);
    kind = "local";
    label = "";
    localPort = "";
    viaHost = "";
    destAddr = "";
    destPort = "";
    remotePort = "";
    error = "";
  }

  function openHostPicker(e: MouseEvent): void {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    px = r.left;
    py = r.bottom + 6;
    hostPickerOpen = true;
  }

  async function save(): Promise<void> {
    if (!viaHost || (kind === "local" && (!localPort || !destAddr || !destPort)) || (kind === "remote" && !remotePort)) {
      error = $t("a.required");
      return;
    }
    busy = true;
    error = "";
    try {
      const bindAddr = kind === "local" ? `${bind}:${localPort}` : `0.0.0.0:${remotePort}`;
      const dest = kind === "local" ? `${destAddr}:${destPort}` : bind;
      await rpc.forwardOpen({ host: viaHost, kind, bind: bindAddr, dest });
      forwardsVersion.update((n) => n + 1);
      toast.success($t("toast.forwardOpened"));
      close();
    } catch (err) {
      error = errorText(err, $t("toast.error"));
    } finally {
      busy = false;
    }
  }
</script>

{#if $forwardEditorOpen}
  <Modal title={$t("fwd.panel")} onClose={close}>
    <div class="psec">
      <div class="seg">
        <button class:on={kind === "local"} on:click={() => (kind = "local")}>{$t("f.local")}</button>
        <button class:on={kind === "remote"} on:click={() => (kind = "remote")}>{$t("f.remote")}</button>
      </div>
      <div class="field">
        <label for="fwd-label">{$t("h.label")}</label>
        <div class="input"><input id="fwd-label" bind:value={label} /></div>
      </div>
      <div class="row">
        {#if kind === "local"}
          <div class="field grow">
            <label for="fwd-lport">{$t("f.lport")} <span class="req">*</span></label>
            <div class="input"><input id="fwd-lport" bind:value={localPort} placeholder="5432" /></div>
          </div>
        {/if}
        <div class="field grow">
          <label for="fwd-bind">{$t("f.bind")}</label>
          <div class="input"><input id="fwd-bind" bind:value={bind} placeholder="127.0.0.1" /></div>
        </div>
      </div>
      <div class="field">
        <label for="fwd-via">{$t("f.via")} <span class="req">*</span></label>
        <button class="picker" id="fwd-via" aria-expanded={hostPickerOpen} on:click={openHostPicker}>
          <Icon name="hosts" />
          <span class="val" class:empty={!viaHost}>{viaHost || $t("f.pick")}</span>
          <Icon name="chev" class="chev" />
        </button>
      </div>
      {#if kind === "local"}
        <div class="row">
          <div class="field grow">
            <label for="fwd-daddr">{$t("f.daddr")} <span class="req">*</span></label>
            <div class="input"><input id="fwd-daddr" bind:value={destAddr} placeholder="db.internal" /></div>
          </div>
          <div class="field port">
            <label for="fwd-dport">{$t("h.port")}</label>
            <div class="input"><input id="fwd-dport" bind:value={destPort} placeholder="5432" /></div>
          </div>
        </div>
      {:else}
        <div class="field">
          <label for="fwd-rport">{$t("f.rport")} <span class="req">*</span></label>
          <div class="input"><input id="fwd-rport" bind:value={remotePort} placeholder="8080" /></div>
        </div>
      {/if}
      {#if error}<p class="error-text">{error}</p>{/if}
    </div>
    <svelte:fragment slot="footer">
      <button class="btn btn-ghost" on:click={close}>{$t("a.cancel")}</button>
      <button class="btn btn-primary" disabled={busy} on:click={save}>{$t("a.save")}</button>
    </svelte:fragment>
  </Modal>
{/if}

{#if hostPickerOpen}
  <Menu x={px} y={py} onClose={() => (hostPickerOpen = false)}>
    {#each $hosts as h (h.id)}
      <button class="menu-item" on:click={() => { viaHost = h.item.name; hostPickerOpen = false; }}>
        <Icon name="hosts" /><span>{h.item.name}</span>
      </button>
    {/each}
  </Menu>
{/if}
