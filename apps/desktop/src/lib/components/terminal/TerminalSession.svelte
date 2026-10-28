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
  import { getTerminalTheme, terminalThemeId } from "../../stores/theme";
  import { registerPaneWriter, unregisterPaneWriter } from "../../stores/terminal";

  export let paneId: string;
  export let hostId: string;
  export let hostName: string;

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
        hostKeyPrompt = { fingerprint: String(e.data ?? "unknown"), mismatch: false };
        status = "error";
      } else if (e.code === ErrorCode.HOST_KEY_MISMATCH) {
        hostKeyPrompt = { fingerprint: String(e.data ?? "unknown"), mismatch: true };
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

  function acceptHostKey(): void {
    if (hostKeyPrompt) void open(hostKeyPrompt.fingerprint);
  }

  onMount(() => {
    term = new Terminal({
      fontFamily: "var(--mono)",
      fontSize: 13,
      cursorBlink: true,
      theme: getTerminalTheme($terminalThemeId),
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
    unsubTheme = terminalThemeId.subscribe((id) => {
      term.options.theme = getTerminalTheme(id);
    });

    void open();
  });

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
  {#if hostKeyPrompt}
    <div class="hostkey-overlay">
      {#if hostKeyPrompt.mismatch}
        <p class="danger-text">
          The host key for <strong>{hostName}</strong> does not match what was trusted before.
          This can mean the server was reinstalled, or that something is intercepting the
          connection. OurShell will not connect automatically.
        </p>
        <code>{hostKeyPrompt.fingerprint}</code>
      {:else}
        <p>
          <strong>{hostName}</strong> presented a host key OurShell has not seen before.
          Verify the fingerprint out-of-band before trusting it:
        </p>
        <code>{hostKeyPrompt.fingerprint}</code>
        <div class="row">
          <button class="primary" on:click={acceptHostKey}>Trust and connect</button>
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .pane-wrap {
    position: relative;
    width: 100%;
    height: 100%;
    background: var(--bg-sunken);
  }
  .term-container {
    width: 100%;
    height: 100%;
    padding: 4px;
  }
  .hostkey-overlay {
    position: absolute;
    inset: 0;
    background: var(--bg-elevated);
    padding: 20px;
    overflow: auto;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .danger-text {
    color: var(--danger);
  }
  code {
    background: var(--bg-sunken);
    padding: 6px;
    border-radius: 4px;
    word-break: break-all;
  }
</style>
