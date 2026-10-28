import { writable } from "svelte/store";
import type { ITheme } from "@xterm/xterm";

export type UiTheme = "dark" | "light";

function loadUiTheme(): UiTheme {
  try {
    const saved = localStorage.getItem("ourshell.ui-theme");
    if (saved === "light" || saved === "dark") return saved;
  } catch {
    /* localStorage unavailable (private mode, etc.) - fall back to dark */
  }
  return "dark";
}

export const uiTheme = writable<UiTheme>(loadUiTheme());

uiTheme.subscribe((t) => {
  document.documentElement.setAttribute("data-theme", t);
  try {
    localStorage.setItem("ourshell.ui-theme", t);
  } catch {
    /* ignore */
  }
});

// ----- Terminal (xterm) color themes -----

export interface TerminalTheme {
  id: string;
  name: string;
  theme: ITheme;
}

export const terminalThemes: TerminalTheme[] = [
  {
    id: "ourshell-dark",
    name: "OurShell Dark",
    theme: {
      background: "#16171d",
      foreground: "#c7c9d1",
      cursor: "#7aa2f7",
      selectionBackground: "#2e3346",
      black: "#1c1e26",
      red: "#f7768e",
      green: "#9ece6a",
      yellow: "#e0af68",
      blue: "#7aa2f7",
      magenta: "#bb9af7",
      cyan: "#7dcfff",
      white: "#c0caf5",
    },
  },
  {
    id: "ourshell-light",
    name: "OurShell Light",
    theme: {
      background: "#ffffff",
      foreground: "#2b2d33",
      cursor: "#2f5fd7",
      selectionBackground: "#cfe0ff",
      black: "#2b2d33",
      red: "#c23b3b",
      green: "#1f8a3d",
      yellow: "#a36a00",
      blue: "#2f5fd7",
      magenta: "#8a3bc2",
      cyan: "#0e8f8f",
      white: "#6b6f7a",
    },
  },
  {
    id: "dracula",
    name: "Dracula",
    theme: {
      background: "#282a36",
      foreground: "#f8f8f2",
      cursor: "#f8f8f0",
      selectionBackground: "#44475a",
      black: "#21222c",
      red: "#ff5555",
      green: "#50fa7b",
      yellow: "#f1fa8c",
      blue: "#bd93f9",
      magenta: "#ff79c6",
      cyan: "#8be9fd",
      white: "#f8f8f2",
    },
  },
  {
    id: "solarized-dark",
    name: "Solarized Dark",
    theme: {
      background: "#002b36",
      foreground: "#839496",
      cursor: "#839496",
      selectionBackground: "#073642",
      black: "#073642",
      red: "#dc322f",
      green: "#859900",
      yellow: "#b58900",
      blue: "#268bd2",
      magenta: "#d33682",
      cyan: "#2aa198",
      white: "#eee8d5",
    },
  },
];

function loadTerminalThemeId(): string {
  try {
    const saved = localStorage.getItem("ourshell.terminal-theme");
    if (saved && terminalThemes.some((t) => t.id === saved)) return saved;
  } catch {
    /* ignore */
  }
  return terminalThemes[0].id;
}

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
