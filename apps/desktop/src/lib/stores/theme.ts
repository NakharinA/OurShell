import { writable } from "svelte/store";
import type { ITheme } from "@xterm/xterm";

// ----- UI theme (light / dark), persisted; defaults to the OS preference -----

export type UiTheme = "dark" | "light";

function loadUiTheme(): UiTheme {
  try {
    const saved = localStorage.getItem("ourshell.ui-theme");
    if (saved === "light" || saved === "dark") return saved;
  } catch {
    /* localStorage unavailable (private mode, etc.) */
  }
  try {
    return matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  } catch {
    return "dark";
  }
}

export const uiTheme = writable<UiTheme>(loadUiTheme());

uiTheme.subscribe((t) => {
  try {
    document.documentElement.setAttribute("data-theme", t);
    localStorage.setItem("ourshell.ui-theme", t);
  } catch {
    /* ignore */
  }
});

// ----- Terminal (xterm) color themes -----
//
// The six swatches from the design blueprint (ssh-client-ui.html
// `#termThemes`): Midnight, Solar, Forest, Paper, Rosé, Ocean. A host's
// `terminal_theme` field stores one of these *names* (not the internal id),
// so it round-trips cleanly through the vault item and the CLI/MCP side
// never needs to know about xterm's ITheme shape.

export interface TerminalTheme {
  id: string;
  name: string;
  theme: ITheme;
}

export const terminalThemes: TerminalTheme[] = [
  {
    id: "midnight",
    name: "Midnight",
    theme: {
      background: "#0b0b0e",
      foreground: "#e4e4e7",
      cursor: "#22c55e",
      selectionBackground: "#2d2d34",
      black: "#0b0b0e",
      red: "#f87171",
      green: "#22c55e",
      yellow: "#eab308",
      blue: "#60a5fa",
      magenta: "#c084fc",
      cyan: "#67e8f9",
      white: "#a1a1aa",
      brightBlack: "#52525b",
      brightWhite: "#f4f4f5",
    },
  },
  {
    id: "solar",
    name: "Solar",
    theme: {
      background: "#002b36",
      foreground: "#93a1a1",
      cursor: "#b58900",
      selectionBackground: "#073642",
      black: "#073642",
      red: "#dc322f",
      green: "#859900",
      yellow: "#b58900",
      blue: "#268bd2",
      magenta: "#d33682",
      cyan: "#2aa198",
      white: "#eee8d5",
      brightBlack: "#586e75",
      brightWhite: "#fdf6e3",
    },
  },
  {
    id: "forest",
    name: "Forest",
    theme: {
      background: "#14231a",
      foreground: "#cbd5c0",
      cursor: "#86efac",
      selectionBackground: "#1f3326",
      black: "#14231a",
      red: "#f87171",
      green: "#86efac",
      yellow: "#facc15",
      blue: "#7dd3fc",
      magenta: "#d8b4fe",
      cyan: "#5eead4",
      white: "#cbd5c0",
      brightBlack: "#3f5a46",
      brightWhite: "#eef5e8",
    },
  },
  {
    id: "paper",
    name: "Paper",
    theme: {
      background: "#f7f5ef",
      foreground: "#3f3a30",
      cursor: "#15803d",
      selectionBackground: "#e5e2d8",
      black: "#3f3a30",
      red: "#b91c1c",
      green: "#15803d",
      yellow: "#a16207",
      blue: "#1d4ed8",
      magenta: "#86198f",
      cyan: "#0f766e",
      white: "#57534e",
      brightBlack: "#78716c",
      brightWhite: "#1c1917",
    },
  },
  {
    id: "rose",
    name: "Rosé",
    theme: {
      background: "#232136",
      foreground: "#e0def4",
      cursor: "#eb6f92",
      selectionBackground: "#2a283e",
      black: "#232136",
      red: "#eb6f92",
      green: "#9ccfd8",
      yellow: "#f6c177",
      blue: "#9ccfd8",
      magenta: "#c4a7e7",
      cyan: "#9ccfd8",
      white: "#e0def4",
      brightBlack: "#6e6a86",
      brightWhite: "#f6f5f9",
    },
  },
  {
    id: "ocean",
    name: "Ocean",
    theme: {
      background: "#0f1c2e",
      foreground: "#cbd5e1",
      cursor: "#38bdf8",
      selectionBackground: "#16263c",
      black: "#0f1c2e",
      red: "#f472b6",
      green: "#34d399",
      yellow: "#fbbf24",
      blue: "#38bdf8",
      magenta: "#f472b6",
      cyan: "#22d3ee",
      white: "#cbd5e1",
      brightBlack: "#3b5068",
      brightWhite: "#f1f5f9",
    },
  },
];

const DEFAULT_THEME_ID = terminalThemes[0].id;

function loadTerminalThemeId(): string {
  try {
    const saved = localStorage.getItem("ourshell.terminal-theme");
    if (saved && terminalThemes.some((t) => t.id === saved)) return saved;
  } catch {
    /* ignore */
  }
  return DEFAULT_THEME_ID;
}

/** Global fallback terminal theme, used when a host has no `terminal_theme` set. */
export const terminalThemeId = writable<string>(loadTerminalThemeId());

terminalThemeId.subscribe((id) => {
  try {
    localStorage.setItem("ourshell.terminal-theme", id);
  } catch {
    /* ignore */
  }
});

export function getTerminalTheme(id: string): ITheme {
  return (terminalThemes.find((t) => t.id === id) ?? terminalThemes[0]).theme;
}

/** Maps a saved host `terminal_theme` *name* (e.g. "Midnight") to an id. */
export function themeIdForName(name: string | null | undefined): string | null {
  if (!name) return null;
  const found = terminalThemes.find((t) => t.name === name);
  return found ? found.id : null;
}
