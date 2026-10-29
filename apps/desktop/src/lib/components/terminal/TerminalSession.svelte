<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { Terminal } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import { WebLinksAddon } from "@xterm/addon-web-links";
  import { writeText, readText } from "@tauri-apps/plugin-clipboard-manager";
  import { rpc } from "../../rpc";
  import { ErrorCode } from "../../types";
  import type { PtyDataEvent, PtyExitEvent } from "../../types";
  import { onAgentEvent } from "../../stores/events";
  import { EventMethod } from "../../types";
  import { getTerminalTheme, terminalThemeId, themeIdForName } from "../../stores/theme";
  import { registerPaneWriter, unregisterPaneWriter } from "../../stores/terminal";
  import { hosts } from "../../stores/items";
  import Modal from "../ui/Modal.svelte";
  import Icon from "../../icons/Icon.svelte";
  import { t, format } from "../../i18n";

  export let paneId: string;
  export let hostId: string;
  export let hostName: string;

  $: hostRecord = $hosts.find((h) => h.id === hostId);
  $: effectiveThemeId = themeIdForName(hostRecord?.item.terminal_theme) ?? $terminalThemeId;
  $: effectiveFontSize = hostRecord?.item.font_size ?? 13;

  let container: HTMLDivElement;
  let term: Terminal;
  let fitAddon: FitAddon;
  let sessionId: string | null = null;
  let status: "connecting" | "open" | "closed" | "error" = "connecting";
  let errorMessage = "";
  let hostKeyPrompt: { fingerprint: string; mismatch: boolean } | null = null;
  let resizeObserver: ResizeObserver | null = null;
  let unsubData: (() => void) | null = null;
  let unsubExit: (() => void) | null = null;
  let unsubTheme: (() => void) | null = null;

  function writeInput(text: string): void {
    if (!sessionId) return;
    void rpc.ptyWrite(sessionId, new TextEncoder().encode(text));
  }

  async function open(acceptHostKey?: string): Promise<void> {
    status = "connecting";
    hostKeyPrompt = null;
    errorMessage = "";
    try {
      const result = await rpc.ptyOpen({
        host: hostId,
        accept_host_key: acceptHostKey ?? null,
        cols: term.cols,
        rows: term.rows,
      });
      sessionId = result.session_id;
      status = "open";
      registerPaneWriter(paneId, writeInput);
      void rpc.ptyResize({ session_id: sessionId, cols: term.cols, rows: term.rows });
    } catch (err) {
      const e = err as { code?: number; message?: string; data?: unknown };
      if (e.code === ErrorCode.HOST_KEY_UNKNOWN) {
        hostKeyPrompt = { fingerprint: hostKeyField(e.data, "fingerprint"), mismatch: false };
        status = "error";
      } else if (e.code === ErrorCode.HOST_KEY_MISMATCH) {
        hostKeyPrompt = { fingerprint: hostKeyField(e.data, "host_port"), mismatch: true };
        status = "error";
        term.write(
          `\r\n\x1b[31mHost key mismatch for ${hostName}: the server's key changed. Refusing to connect.\x1b[0m\r\n`,
        );
      } else {
        status = "error";
        errorMessage = e.message ?? String(err);
        term.write(`\r\n\x1b[31mFailed to connect: ${errorMessage}\x1b[0m\r\n`);
      }
    }
  }

  /** Agent error data is `{host_port, key, fingerprint}` (see crates/cli/src/agent/ssh_ops.rs). */
  function hostKeyField(data: unknown, field: "fingerprint" | "host_port"): string {
    if (data && typeof data === "object" && field in data) {
      return String((data as Record<string, unknown>)[field]);
    }
    return typeof data === "string" ? data : "unknown";
  }

  function acceptHostKey(): void {
    if (hostKeyPrompt) void open(hostKeyPrompt.fingerprint);
  }

  onMount(() => {
    term = new Terminal({
      fontFamily: "var(--mono)",
      fontSize: effectiveFontSize,
      cursorBlink: true,
      theme: getTerminalTheme(effectiveThemeId),
      scrollback: 5000,
    });
    fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.loadAddon(new WebLinksAddon());
    term.open(container);
    fitAddon.fit();

    term.onData((data) => {
      if (status === "open") writeInput(data);
    });

    // Copy on select.
    term.onSelectionChange(() => {
      const sel = term.getSelection();
      if (sel) void writeText(sel);
    });

    // Paste via Ctrl+Shift+V.
    term.attachCustomKeyEventHandler((ev) => {
      if (ev.type === "keydown" && ev.ctrlKey && ev.shiftKey && ev.key.toLowerCase() === "v") {
        void readText().then((text) => {
          if (text) writeInput(text);
        });
        return false;
      }
      return true;
    });

    resizeObserver = new ResizeObserver(() => {
      fitAddon.fit();
      if (sessionId) {
        void rpc.ptyResize({ session_id: sessionId, cols: term.cols, rows: term.rows });
      }
    });
    resizeObserver.observe(container);

    unsubData = onAgentEvent(EventMethod.PTY_DATA, (p) => {
      const ev = p as PtyDataEvent;
      if (ev.session_id === sessionId) {
        term.write(Uint8Array.from(atob(ev.data), (c) => c.charCodeAt(0)));
      }
    });
    unsubExit = onAgentEvent(EventMethod.PTY_EXIT, (p) => {
      const ev = p as PtyExitEvent;
      if (ev.session_id === sessionId) {
        status = "closed";
        term.write(`\r\n\x1b[33m[process exited${ev.exit_code != null ? ` (${ev.exit_code})` : ""}]\x1b[0m\r\n`);
      }
    });
    unsubTheme = terminalThemeId.subscribe(() => {
      term.options.theme = getTerminalTheme(effectiveThemeId);
    });

    void open();
  });

  // Host-specific theme/font size (set in the host modal) override the
  // global default; re-apply live if either changes.
  $: if (term) {
    term.options.theme = getTerminalTheme(effectiveThemeId);
    term.options.fontSize = effectiveFontSize;
    fitAddon?.fit();
  }

  onDestroy(() => {
    unsubData?.();
    unsubExit?.();
    unsubTheme?.();
    resizeObserver?.disconnect();
    unregisterPaneWriter(paneId);
    if (sessionId) {
      void rpc.ptyClose({ session_id: sessionId });
    }
    term?.dispose();
  });
</script>

<div class="pane-wrap">
  <div class="term-container" bind:this={container}></div>
</div>

{#if hostKeyPrompt}
  <Modal
    title={hostKeyPrompt.mismatch ? $t("term.hostkeyMismatch") : $t("term.hostkeyUnknown")}
    onClose={() => (hostKeyPrompt = null)}
  >
    <div class="psec">
      {#if hostKeyPrompt.mismatch}
        <div class="alert" style="background:var(--danger-soft);color:var(--danger-text);align-items:flex-start">
          <Icon name="warn" />
          <span>{format($t("term.hostkeyMismatchBody"), { host: hostName })}</span>
        </div>
      {:else}
        <p class="hint">{format($t("term.hostkeyUnknownBody"), { host: hostName })}</p>
      {/if}
      <pre class="code">{hostKeyPrompt.fingerprint}</pre>
    </div>
    <svelte:fragment slot="footer">
      <button class="btn btn-ghost" on:click={() => (hostKeyPrompt = null)}>{$t("a.cancel")}</button>
      {#if !hostKeyPrompt.mismatch}
        <button class="btn btn-primary" on:click={acceptHostKey}>{$t("term.trust")}</button>
      {/if}
    </svelte:fragment>
  </Modal>
{/if}

<style>
  .pane-wrap {
    position: relative;
    width: 100%;
    height: 100%;
    background: var(--bg-page);
  }
  .term-container {
    width: 100%;
    height: 100%;
    padding: 4px;
  }
</style>
