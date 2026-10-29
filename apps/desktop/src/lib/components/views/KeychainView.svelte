<!-- Keychain (design `data-view="keychain"`). -->
<script lang="ts">
  import Icon from "../../icons/Icon.svelte";
  import Menu from "../ui/Menu.svelte";
  import { t } from "../../i18n";
  import { keys, refreshKeys } from "../../stores/items";
  import { keyEditor } from "../../stores/ui";
  import { rpc } from "../../rpc";
  import { toast, errorText } from "../../stores/toast";
  import type { ItemRecord, SshKey } from "../../types";

  let menu: { x: number; y: number; kind: "new" | "item"; record?: ItemRecord<SshKey> } | null = null;

  function algoOf(pub: string): string {
    const head = pub.trim().split(/\s+/)[0] ?? "";
    if (head.includes("ed25519")) return "ED25519";
    if (head.includes("rsa")) return "RSA";
    if (head.includes("ecdsa")) return "ECDSA";
    return head || "—";
  }

  function openMenu(kind: "new" | "item", e: MouseEvent, record?: ItemRecord<SshKey>): void {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    menu = { x: r.right, y: r.bottom + 6, kind, record };
  }

  function copyPub(k: ItemRecord<SshKey>): void {
    void navigator.clipboard.writeText(k.item.public_key).then(() => toast.success($t("keys.pubCopied")));
    menu = null;
  }

  async function remove(k: ItemRecord<SshKey>): Promise<void> {
    menu = null;
    if (!confirm(`${$t("a.remove")} "${k.item.name}"?`)) return;
    try {
      await rpc.itemDelete({ kind: "key", id: k.id });
      await refreshKeys();
      toast.success($t("toast.keyDeleted"));
    } catch (err) {
      toast.error(errorText(err, $t("toast.error")));
    }
  }
</script>

<section class="view">
  <div class="card-head">
    <div class="title-row">
      <h1 class="card-title">{$t("keys.title")}</h1>
      <span class="badge accent">{$keys.length}</span>
    </div>
    <div class="toolbar">
      <div class="split">
        <button class="btn btn-primary" on:click={() => keyEditor.set({ mode: "new", kind: "generate" })}>
          <Icon name="plus" /><span>{$t("keys.new")}</span>
        </button>
        <button class="btn btn-primary" aria-label="More" on:click={(e) => openMenu("new", e)}>
          <Icon name="chev" />
        </button>
      </div>
      <div class="spacer"></div>
    </div>
  </div>
  <div class="card-body">
    {#if $keys.length === 0}
      <div class="empty">
        <Icon name="key" size="lg" />
        <strong>{$t("keys.empty")}</strong>
        <p>{$t("keys.emptyHint")}</p>
      </div>
    {:else}
      <div class="grid">
        {#each $keys as k (k.id)}
          <div class="item" role="button" tabindex="0">
            <span class="round"><Icon name="key" /></span>
            <span class="meta">
              <div class="item-title">{k.item.name}</div>
              <div class="item-sub">{algoOf(k.item.public_key)}</div>
            </span>
            <span class="item-actions">
              <button class="icon-btn sm" aria-label="More" on:click|stopPropagation={(e) => openMenu("item", e, k)}>
                <Icon name="dots" />
              </button>
            </span>
          </div>
        {/each}
      </div>
    {/if}
  </div>
</section>

{#if menu?.kind === "new"}
  <Menu x={menu.x} y={menu.y} align="right" onClose={() => (menu = null)}>
    <button class="menu-item" on:click={() => { keyEditor.set({ mode: "new", kind: "generate" }); menu = null; }}>
      <Icon name="key" /><span>{$t("keys.generate")}</span>
    </button>
    <button class="menu-item" on:click={() => { keyEditor.set({ mode: "new", kind: "import" }); menu = null; }}>
      <Icon name="import" /><span>{$t("keys.importFile")}</span>
    </button>
  </Menu>
{:else if menu?.kind === "item" && menu.record}
  <Menu x={menu.x} y={menu.y} align="right" onClose={() => (menu = null)}>
    <button class="menu-item" on:click={() => menu?.record && copyPub(menu.record)}>
      <Icon name="copy" /><span>{$t("a.copy")} {$t("k.public")}</span>
    </button>
    <div class="menu-sep"></div>
    <button class="menu-item danger" on:click={() => menu?.record && remove(menu.record)}>
      <Icon name="trash" /><span>{$t("a.remove")}</span>
    </button>
  </Menu>
{/if}
