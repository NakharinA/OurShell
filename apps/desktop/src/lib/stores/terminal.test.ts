import { beforeEach, describe, expect, it } from "vitest";
import { get } from "svelte/store";
import { activeTabId, openHostTab, openOrFocusHostTab, tabs } from "./terminal";

describe("openOrFocusHostTab", () => {
  beforeEach(() => {
    tabs.set([]);
    activeTabId.set(null);
  });

  it("reuses the existing tab for the same host", () => {
    const first = openOrFocusHostTab("h1", "web");
    openOrFocusHostTab("h2", "db");
    const again = openOrFocusHostTab("h1", "web");
    expect(again).toBe(first);
    expect(get(tabs)).toHaveLength(2);
    expect(get(activeTabId)).toBe(first);
  });

  it("openHostTab always opens another tab", () => {
    const a = openHostTab("h1", "web");
    const b = openHostTab("h1", "web");
    expect(a).not.toBe(b);
    expect(get(tabs)).toHaveLength(2);
    // A plain click then goes to the first one.
    expect(openOrFocusHostTab("h1", "web")).toBe(a);
  });
});
