<!-- New/edit snippet (design `data-modal="snippets"`). Running it executes
     `exec.run` on each target host in parallel and shows a result card per
     host (exit code, stdout/stderr) in the design's `.code` style. -->
<script lang="ts">
  import Modal from "../ui/Modal.svelte";
  import Icon from "../../icons/Icon.svelte";
  import Menu from "../ui/Menu.svelte";
  import { t } from "../../i18n";
  import { rpc } from "../../rpc";
  import { isTelnet, type Snippet } from "../../types";
  import { hosts, refreshSnippets } from "../../stores/items";
  import { snippetEditor } from "../../stores/ui";
  import { toast, errorText } from "../../stores/toast";

  $: state = $snippetEditor;
  $: record = state?.mode === "edit" ? state.record : null;

  let form: Snippet = { name: "", script: "" };
  let targets: string[] = [];
  let error = "";
  let busy = false;
  let running = false;
  let targetPickerOpen = false;
  let px = 0;
  let py = 0;
  let lastKey: string | null | undefined;
  let results: { host: string; exit_code: number | null; stdout: string; stderr: string; error?: string }[] = [];

  $: initFrom(state);
  function initFrom(s: typeof state): void {
    if (!s) return;
    const key = s.mode === "edit" ? s.record.id : "new";
    if (key === lastKey) return;
    lastKey = key;
    form = s.mode === "edit" ? { ...s.record.item } : { name: "", script: "" };
    targets = [];
    results = [];
    error = "";
  }

  function close(): void {
    snippetEditor.set(null);
    lastKey = null;
  }

  function openTargetPicker(e: MouseEvent): void {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    px = r.left;
    py = r.bottom + 6;
    targetPickerOpen = true;
  }
  function addTarget(name: string): void {
    if (!targets.includes(name)) targets = [...targets, name];
    targetPickerOpen = false;
  }
  function removeTarget(name: string): void {
    targets = targets.filter((t2) => t2 !== name);
  }

  async function save(): Promise<void> {
    if (!form.name || !form.script) {
      error = $t("a.required");
      return;
    }
    busy = true;
    error = "";
    try {
      await rpc.itemPut<Snippet>({ kind: "snippet", id: record?.id ?? null, item: form });
      await refreshSnippets();
      toast.success($t("toast.snippetSaved"));
      close();
    } catch (err) {
      error = errorText(err, $t("toast.error"));
    } finally {
      busy = false;
    }
  }

  async function run(): Promise<void> {
    if (!form.script || targets.length === 0) {
      error = $t("a.required");
      return;
    }
    running = true;
    error = "";
    results = targets.map((host) => ({ host, exit_code: null, stdout: "", stderr: "" }));
    await Promise.all(
      targets.map(async (host, i) => {
        try {
          const res = await rpc.execRun({ host, command: form.script });
          results[i] = { host, exit_code: res.exit_code, stdout: res.stdout, stderr: res.stderr };
        } catch (err) {
          results[i] = { host, exit_code: null, stdout: "", stderr: "", error: errorText(err, $t("toast.error")) };
        }
        results = results;
      }),
    );
    running = false;
  }
</script>

{#if state}
  <Modal title={record ? $t("snip.edit") : $t("snip.panel")} size="wide" onClose={close}>
    <div class="psec">
      <div class="field">
        <label for="snip-name">{$t("h.label")}</label>
        <div class="input"><input id="snip-name" bind:value={form.name} /></div>
      </div>
      <div class="field">
        <label for="snip-script">{$t("s.script")} <span class="req">*</span></label>
        <div class="textarea"><textarea id="snip-script" class="mono" style="min-height:120px" bind:value={form.script} placeholder="#!/bin/bash"></textarea></div>
      </div>
    </div>
    <div class="psec">
      <h3><Icon name="hosts" /><span>{$t("s.targets")}</span></h3>
      <p class="hint">{$t("s.targetsHint")}</p>
      {#if targets.length}
        <div class="chips">
          {#each targets as name (name)}
            <span class="chip">
              <Icon name="hosts" /><span>{name}</span>
              <button class="chip-x" aria-label="Remove" on:click={() => removeTarget(name)}><Icon name="x" size="sm" /></button>
            </span>
          {/each}
        </div>
      {/if}
      <button class="add-link" aria-expanded={targetPickerOpen} on:click={openTargetPicker}>
        <Icon name="plus" /><span>{$t("s.addTargets")}</span>
      </button>
    </div>
    {#if results.length}
      <div class="psec">
        <h3><Icon name="checkc" /><span>{$t("snip.results")}</span></h3>
        {#each results as r (r.host)}
          <div class="field">
            <div class="row">
              <b>{r.host}</b>
              {#if r.error}
                <span class="badge err">{$t("r.error")}</span>
              {:else if r.exit_code === null}
                <span class="badge warn">{$t("snip.running")}</span>
              {:else}
                <span class="badge {r.exit_code === 0 ? 'ok' : 'err'}">{$t("snip.exit")}: {r.exit_code}</span>
              {/if}
            </div>
            <pre class="code">{r.error ?? `${r.stdout}${r.stderr ? `\n${r.stderr}` : ""}`}</pre>
          </div>
        {/each}
      </div>
    {/if}
    {#if error}<p class="error-text" style="padding:0 24px">{error}</p>{/if}
    <svelte:fragment slot="footer">
      <button class="btn btn-ghost" on:click={close}>{$t("a.cancel")}</button>
      <button class="btn btn-soft" disabled={busy || running} on:click={save}>{$t("a.save")}</button>
      <button class="btn btn-primary" disabled={busy || running} on:click={run}>
        <Icon name="play" /><span>{$t("a.run")}</span>
      </button>
    </svelte:fragment>
  </Modal>
{/if}

{#if targetPickerOpen}
  <Menu x={px} y={py} onClose={() => (targetPickerOpen = false)}>
    {#each $hosts.filter((h) => !targets.includes(h.item.name) && !isTelnet(h.item)) as h (h.id)}
      <button class="menu-item" on:click={() => addTarget(h.item.name)}>
        <Icon name="hosts" /><span>{h.item.name}</span>
      </button>
    {/each}
  </Menu>
{/if}
