import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ readText: vi.fn(), writeText: vi.fn() }));
import { clipboardAction } from "./terminalClipboard";

const key = (k: string, mods: Partial<{ ctrl: boolean; shift: boolean; alt: boolean }> = {}) => ({
  type: "keydown",
  key: k,
  ctrlKey: !!mods.ctrl,
  shiftKey: !!mods.shift,
  altKey: !!mods.alt,
  metaKey: false,
});

describe("clipboardAction", () => {
  it("Ctrl+C copies only when text is selected", () => {
    expect(clipboardAction(key("c", { ctrl: true }), true)).toBe("copy");
    expect(clipboardAction(key("c", { ctrl: true }), false)).toBeNull(); // ^C to the shell
  });
  it("Ctrl+Shift+C always copies", () => {
    expect(clipboardAction(key("C", { ctrl: true, shift: true }), false)).toBe("copy");
  });
  it("Ctrl+V, Ctrl+Shift+V and Shift+Insert paste", () => {
    expect(clipboardAction(key("v", { ctrl: true }), false)).toBe("paste");
    expect(clipboardAction(key("V", { ctrl: true, shift: true }), false)).toBe("paste");
    expect(clipboardAction(key("Insert", { shift: true }), false)).toBe("paste");
  });
  it("leaves other keys to the terminal", () => {
    expect(clipboardAction(key("d", { ctrl: true }), true)).toBeNull();
    expect(clipboardAction(key("v"), false)).toBeNull();
    expect(clipboardAction(key("v", { ctrl: true, alt: true }), false)).toBeNull();
    expect(clipboardAction({ ...key("v", { ctrl: true }), type: "keyup" }, false)).toBeNull();
  });
});
