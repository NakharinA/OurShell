<!-- Host editor (design `data-modal="hosts"`). Credentials are stored behind
     the scenes as an Identity (see ../../identity.ts) - this UI never shows
     the word "Identity". -->
<script lang="ts">
  import { confirmDialog } from "../../stores/confirm";
  import Modal from "../ui/Modal.svelte";
  import Icon from "../../icons/Icon.svelte";
  import Menu from "../ui/Menu.svelte";
  import { t } from "../../i18n";
  import { rpc } from "../../rpc";
  import { emptyHost, type Host, type Identity } from "../../types";
  import { groups, hosts, keys, refreshHosts, refreshKeys } from "../../stores/items";
  import { hostEditor, keyEditor, keyPickerReturn } from "../../stores/ui";
  import { openHostTab } from "../../stores/terminal";
  import { focusTerminalTab } from "../../stores/windowTabs";
  import { terminalThemes } from "../../stores/theme";
  import { currentVaultId } from "../../stores/vaultContext";
  import { teams, vaultNames } from "../../stores/teams";
  import { planIdentityUpdate } from "../../identity";
  import { toast, errorText } from "../../stores/toast";

  $: state = $hostEditor;
  $: record = state?.mode === "edit" ? state.record : null;

  let form: Host = emptyHost();
  let tagsText = "";
  let tagInput = "";
  let password = "";
  let showPassword = false;
  let keyId: string | null = null;
  let existingIdentityId: string | null = null;
  let error = "";
  let busy = false;
  let lastKey: string | null | undefined;

  let groupPickerOpen = false;
  let jumpPickerOpen = false;
  let keyMenuOpen = false;
  let themePicker = "";
  let headerMenuOpen = false;
  let px = 0;
  let py = 0;

  $: initFromState(state);

  async function initFromState(s: typeof state): Promise<void> {
    if (!s) return;
    const key = s.mode === "edit" ? s.record.id : "new";
    if (key === lastKey) return;
    lastKey = key;
    password = "";
    keyId = null;
    existingIdentityId = null;
    error = "";
    tagInput = "";
    if (s.mode === "edit") {
      form = { ...s.record.item };
      existingIdentityId = form.identity_id;
      if (existingIdentityId) {
        try {
          const ident = await rpc.itemGet<Identity>({ kind: "identity", id: existingIdentityId });
          password = ident.item.password ?? "";
          keyId = ident.item.key_id ?? null;
        } catch {
          /* linked identity vanished - leave credentials blank */
        }
      }
    } else {
      form = emptyHost();
      if (s.groupId) form.group_id = s.groupId;
      if (s.prefill) form = { ...form, ...s.prefill };
    }
    tagsText = form.tags.join(", ");
  }

  function close(): void {
    hostEditor.set(null);
    lastKey = null;
  }

  function openPicker(which: "group" | "jump" | "keys", e: MouseEvent): void {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    px = r.left;
    py = r.bottom + 6;
    groupPickerOpen = which === "group";
    jumpPickerOpen = which === "jump";
    keyMenuOpen = which === "keys";
  }

  function addTag(): void {
    const v = tagInput.trim();
    if (v && !form.tags.includes(v)) form.tags = [...form.tags, v];
    tagInput = "";
  }
  function removeTag(tag: string): void {
    form.tags = form.tags.filter((x) => x !== tag);
  }

  function pickKey(id: string | null): void {
    keyId = id;
    keyMenuOpen = false;
  }
  function openGenerateKey(): void {
    keyMenuOpen = false;
    keyPickerReturn.set((id) => {
      keyId = id;
    });
    keyEditor.set({ mode: "new" });
  }

  $: selectedKeyName = keyId ? $keys.find((k) => k.id === keyId)?.item.name ?? "" : "";
  $: groupName = form.group_id ? $groups.find((g) => g.id === form.group_id)?.item.name ?? "" : "";
  $: jumpHosts = $hosts.filter((h) => h.id !== record?.id);

  async function persistIdentity(name: string): Promise<string | null> {
    const plan = planIdentityUpdate({
      hostName: name,
      username: form.username,
      password,
      keyId,
      existingIdentityId,
    });
    if (plan.action === "none") return existingIdentityId;
    if (plan.action === "clear") {
      if (plan.deleteId) {
        try {
          await rpc.itemDelete({ kind: "identity", id: plan.deleteId });
        } catch {
          /* best effort */
        }
      }
      return null;
    }
    const saved = await rpc.itemPut<Identity>({ kind: "identity", id: plan.id, item: plan.identity });
    return saved.id;
  }

  async function doSave(connect: boolean): Promise<void> {
    if (!form.name || !form.hostname) {
      error = $t("a.required");
      return;
    }
    busy = true;
    error = "";
    try {
      form.tags = tagsText
        ? tagsText.split(",").map((t2) => t2.trim()).filter(Boolean)
        : form.tags;
      form.identity_id = await persistIdentity(form.name);
      const saved = await rpc.itemPut<Host>({ kind: "host", id: record?.id ?? null, item: form });
      if ($currentVaultId && !record) {
        try {
          await rpc.teamShare({ item_id: saved.id, team: $currentVaultId });
        } catch {
          /* sharing is best-effort from this form; the host is still saved personally */
        }
      }
      await refreshHosts();
      if (connect) {
        const tabId = openHostTab(saved.id, form.name);
        focusTerminalTab(tabId);
      }
      close();
    } catch (err) {
      error = errorText(err, $t("toast.error"));
    } finally {
      busy = false;
    }
  }

  async function duplicate(): Promise<void> {
    headerMenuOpen = false;
    if (!record) return;
    try {
      await rpc.itemPut<Host>({ kind: "host", id: null, item: { ...record.item, name: `${record.item.name}-copy` } });
      await refreshHosts();
      close();
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    }
  }
  async function remove(): Promise<void> {
    headerMenuOpen = false;
    if (!record) return;
    if (!(await confirmDialog({ title: `${$t("a.remove")} "${record.item.name}"?` }))) return;
    busy = true;
    try {
      await rpc.itemDelete({ kind: "host", id: record.id });
      await refreshHosts();
      close();
    } catch (err) {
      error = errorText(err, $t("toast.error"));
    } finally {
      busy = false;
    }
  }
</script>

{#if state}
  <Modal title={record ? $t("hosts.edit") : $t("hosts.panel")} size="wide" onClose={close}>
    <svelte:fragment slot="headerExtra">
      <button class="vault-chip">
        <Icon name="vault" />
        <span>
          {record?.vault_id
            ? $vaultNames.get(record.vault_id) ?? "Team"
            : $currentVaultId
              ? $teams.find((v) => v.vault_id === $currentVaultId)?.name ?? "Team"
              : $t("vault.personal")}
        </span>
      </button>
      {#if record}
        <button
          class="icon-btn sm"
          aria-expanded={headerMenuOpen}
          aria-label="More"
          on:click={(e) => {
            const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
            px = r.right;
            py = r.bottom + 6;
            headerMenuOpen = !headerMenuOpen;
          }}
        >
          <Icon name="dots" />
        </button>
      {/if}
    </svelte:fragment>

    <div class="psec">
      <h3><Icon name="globe" /><span>{$t("h.address")}</span></h3>
      <div class="row">
        <div class="field grow">
          <label for="h-hostname">{$t("h.hostname")}</label>
          <div class="input"><Icon name="hosts" /><input id="h-hostname" bind:value={form.hostname} placeholder="192.168.1.10" /></div>
        </div>
        <div class="field port">
          <label for="h-port">{$t("h.port")}</label>
          <div class="input"><input id="h-port" type="number" bind:value={form.port} placeholder="22" /></div>
        </div>
      </div>
    </div>

    <div class="psec">
      <h3><Icon name="tag" /><span>{$t("h.general")}</span></h3>
      <div class="field">
        <label for="h-label">{$t("h.label")}</label>
        <div class="input"><input id="h-label" bind:value={form.name} placeholder={$t("h.labelPh")} /></div>
      </div>
      <div class="row">
        <div class="field grow">
          <label for="h-group">{$t("h.group")}</label>
          <button class="picker" id="h-group" aria-expanded={groupPickerOpen} on:click={(e) => openPicker("group", e)}>
            <Icon name="group" />
            <span class="val" class:empty={!groupName}>{groupName || $t("h.none")}</span>
            <Icon name="chev" class="chev" />
          </button>
        </div>
        <div class="field grow">
          <label for="h-tags">{$t("h.tags")}</label>
          <div class="input">
            <Icon name="tag" />
            <input
              id="h-tags"
              bind:value={tagInput}
              placeholder={$t("h.addTag")}
              on:keydown={(e) => e.key === "Enter" && (e.preventDefault(), addTag())}
            />
          </div>
        </div>
      </div>
      {#if form.tags.length}
        <div class="chips">
          {#each form.tags as tag (tag)}
            <span class="chip">
              <span>{tag}</span>
              <button class="chip-x" aria-label="Remove" on:click={() => removeTag(tag)}><Icon name="x" size="sm" /></button>
            </span>
          {/each}
        </div>
      {/if}
      <div class="field">
        <label for="h-jump">{$t("h.jump")}</label>
        <button class="picker" id="h-jump" aria-expanded={jumpPickerOpen} on:click={(e) => openPicker("jump", e)}>
          <Icon name="route" />
          <span class="val" class:empty={!form.jump_host}>{form.jump_host || $t("h.none")}</span>
          <Icon name="chev" class="chev" />
        </button>
      </div>
    </div>

    <div class="psec">
      <h3><Icon name="shield" /><span>{$t("h.creds")}</span></h3>
      <div class="field">
        <label for="h-user">{$t("h.user")}</label>
        <div class="input"><Icon name="user" /><input id="h-user" bind:value={form.username} placeholder="root" /></div>
      </div>
      <div class="field">
        <label for="h-pass">{$t("h.pass")}</label>
        <div class="input">
          <Icon name="pass" />
          {#if showPassword}
            <input id="h-pass" bind:value={password} placeholder="••••••••" />
          {:else}
            <input id="h-pass" type="password" bind:value={password} placeholder="••••••••" />
          {/if}
          <button type="button" title={$t("h.show")} on:click={() => (showPassword = !showPassword)}>
            <Icon name={showPassword ? "eye" : "eyeoff"} size="sm" />
          </button>
        </div>
      </div>
      <div class="field">
        <span class="lbl">{$t("h.keys")}</span>
        {#if selectedKeyName}
          <div class="chips">
            <span class="chip">
              <Icon name="key" />
              <span>{selectedKeyName}</span>
              <button class="chip-x" aria-label="Remove" on:click={() => (keyId = null)}><Icon name="x" size="sm" /></button>
            </span>
          </div>
        {/if}
      </div>
      <button class="add-link" aria-expanded={keyMenuOpen} on:click={(e) => openPicker("keys", e)}>
        <Icon name="plus" /><span>{$t("h.addKey")}</span>
      </button>
      <div class="toggle-row">
        <span class="grow"><b>{$t("h.agent")}</b><small>{$t("h.agentSub")}</small></span>
        <label class="switch"><input type="checkbox" bind:checked={form.agent_forwarding} /><span></span></label>
      </div>
    </div>

    <div class="psec">
      <h3><Icon name="shieldok" /><span>{$t("h.ai")}</span></h3>
      <div class="toggle-row">
        <span class="grow"><b>{$t("h.aiAllow")}</b><small>{$t("h.aiAllowSub")}</small></span>
        <label class="switch"><input type="checkbox" bind:checked={form.ai_allowed} /><span></span></label>
      </div>
      {#if form.ai_allowed}
        <div class="field">
          <label for="h-aiuser">{$t("h.aiUser")}</label>
          <div class="input"><input id="h-aiuser" bind:value={form.ai_user} placeholder={$t("h.aiUserPh")} /></div>
        </div>
        <div class="toggle-row">
          <span class="grow"><b>{$t("h.aiReadonlyAuto")}</b><small>{$t("h.aiReadonlyAutoSub")}</small></span>
          <label class="switch"><input type="checkbox" bind:checked={form.ai_readonly_auto} /><span></span></label>
        </div>
      {/if}
    </div>

    <div class="psec">
      <h3><Icon name="monitor" /><span>{$t("h.theme")}</span><span class="spacer"></span><span class="badge accent">{form.terminal_theme ?? terminalThemes[0].name}</span></h3>
      <div class="themes">
        {#each terminalThemes as th (th.id)}
          <button
            class="theme-sw"
            class:selected={(form.terminal_theme ?? terminalThemes[0].name) === th.name}
            on:click={() => (form.terminal_theme = th.name)}
          >
            <span class="sw-prev" style="background:{th.theme.background}">
              <i style="background:{th.theme.green};width:45%"></i>
              <i style="background:{th.theme.white};width:75%"></i>
              <i style="background:{th.theme.blue};width:55%"></i>
            </span>
            <span class="sw-name">{th.name}</span>
          </button>
        {/each}
      </div>
      <div class="row">
        <span class="hint grow">{$t("h.font")}</span>
        <div class="input port"><input type="number" bind:value={form.font_size} placeholder="14" /></div>
        <span class="hint">px</span>
      </div>
    </div>

    <svelte:fragment slot="footer">
      {#if error}<span class="error-text foot-left">{error}</span>{/if}
      <button class="btn btn-ghost" on:click={close}>{$t("a.cancel")}</button>
      <button class="btn btn-soft" disabled={busy} on:click={() => doSave(false)}>{$t("a.save")}</button>
      <button class="btn btn-primary" disabled={busy} on:click={() => doSave(true)}>
        <Icon name="terminal" /><span>{$t("a.saveConnect")}</span>
      </button>
    </svelte:fragment>
  </Modal>
{/if}

{#if groupPickerOpen}
  <Menu x={px} y={py} onClose={() => (groupPickerOpen = false)}>
    <button class="menu-item" on:click={() => { form.group_id = null; groupPickerOpen = false; }}>
      <span>{$t("h.none")}</span>
    </button>
    {#each $groups as g (g.id)}
      <button class="menu-item" on:click={() => { form.group_id = g.id; groupPickerOpen = false; }}>
        <Icon name="group" /><span>{g.item.name}</span>
      </button>
    {/each}
  </Menu>
{:else if jumpPickerOpen}
  <Menu x={px} y={py} onClose={() => (jumpPickerOpen = false)}>
    <button class="menu-item" on:click={() => { form.jump_host = null; jumpPickerOpen = false; }}>
      <span>{$t("h.none")}</span>
    </button>
    {#each jumpHosts as h (h.id)}
      <button class="menu-item" on:click={() => { form.jump_host = h.item.name; jumpPickerOpen = false; }}>
        <Icon name="hosts" /><span>{h.item.name}</span>
      </button>
    {/each}
  </Menu>
{:else if keyMenuOpen}
  <Menu x={px} y={py} onClose={() => (keyMenuOpen = false)}>
    {#each $keys as k (k.id)}
      <button class="menu-item" on:click={() => pickKey(k.id)}>
        <Icon name="key" /><span>{k.item.name}</span>
      </button>
    {/each}
    <div class="menu-sep"></div>
    <button class="menu-item" on:click={openGenerateKey}>
      <Icon name="plus" /><span>{$t("h.genKey")}</span>
    </button>
  </Menu>
{:else if headerMenuOpen}
  <Menu x={px} y={py} align="right" onClose={() => (headerMenuOpen = false)}>
    <button class="menu-item" on:click={duplicate}>
      <Icon name="copy" /><span>{$t("h.duplicate")}</span>
    </button>
    <div class="menu-sep"></div>
    <button class="menu-item danger" on:click={remove}>
      <Icon name="trash" /><span>{$t("h.delete")}</span>
    </button>
  </Menu>
{/if}
