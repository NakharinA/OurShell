<script lang="ts">
  import Modal from "../ui/Modal.svelte";
  import Icon from "../../icons/Icon.svelte";
  import Menu from "../ui/Menu.svelte";
  import { t } from "../../i18n";
  import { rpc } from "../../rpc";
  import { groupEditor } from "../../stores/ui";
  import { groups, refreshGroups } from "../../stores/items";
  import { toast, errorText } from "../../stores/toast";
  import type { Group } from "../../types";
  import { groupPath, parentCandidates } from "../../groupTree";

  $: state = $groupEditor;
  $: record = state?.mode === "edit" ? state.record : null;

  let name = "";
  let parentId: string | null = null;
  let busy = false;
  let pickerOpen = false;
  let pickerX = 0;
  let pickerY = 0;
  let lastKey: string | null = null;

  $: initFrom(state);
  function initFrom(s: typeof state): void {
    if (!s) return;
    const key = s.mode === "edit" ? s.record.id : "new";
    if (key === lastKey) return;
    lastKey = key;
    if (s.mode === "edit") {
      name = s.record.item.name;
      parentId = s.record.item.parent_id;
    } else {
      name = "";
      parentId = s.parentId ?? null;
    }
  }

  function close(): void {
    groupEditor.set(null);
    lastKey = null;
  }

  async function save(): Promise<void> {
    if (!name.trim()) return;
    busy = true;
    try {
      const item: Group = { name: name.trim(), parent_id: parentId };
      await rpc.itemPut<Group>({ kind: "group", id: record?.id ?? null, item });
      await refreshGroups();
      close();
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    } finally {
      busy = false;
    }
  }

  function openPicker(e: MouseEvent): void {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    pickerX = r.left;
    pickerY = r.bottom + 6;
    pickerOpen = true;
  }
  $: parentName = parentId ? $groups.find((g) => g.id === parentId)?.item.name ?? "" : "";
</script>

{#if state}
  <Modal title={record ? $t("grp.edit") : $t("mn.group")} onClose={close}>
    <div class="psec">
      <div class="field">
        <label for="group-name">{$t("mn.groupName")}</label>
        <div class="input">
          <input id="group-name" bind:value={name} on:keydown={(e) => e.key === "Enter" && save()} />
        </div>
      </div>
      <div class="field">
        <label for="group-parent">{$t("mn.groupParent")}</label>
        <button class="picker" id="group-parent" aria-expanded={pickerOpen} on:click={openPicker}>
          <Icon name="group" />
          <span class="val" class:empty={!parentName}>{parentName || $t("h.none")}</span>
          <Icon name="chev" class="chev" />
        </button>
      </div>
    </div>
    <svelte:fragment slot="footer">
      <button class="btn btn-ghost" on:click={close}>{$t("a.cancel")}</button>
      <button class="btn btn-primary" disabled={busy || !name.trim()} on:click={save}>{$t("a.save")}</button>
    </svelte:fragment>
  </Modal>
{/if}

{#if pickerOpen}
  <Menu x={pickerX} y={pickerY} onClose={() => (pickerOpen = false)}>
    <button class="menu-item" on:click={() => { parentId = null; pickerOpen = false; }}>
      <span>{$t("h.none")}</span>
    </button>
    {#each parentCandidates($groups, record?.id ?? null) as g (g.id)}
      <button class="menu-item" on:click={() => { parentId = g.id; pickerOpen = false; }}>
        <Icon name="group" /><span>{groupPath($groups, g.id).map((x) => x.item.name).join(" / ")}</span>
      </button>
    {/each}
  </Menu>
{/if}
