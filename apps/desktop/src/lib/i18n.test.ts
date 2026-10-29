import { describe, expect, it } from "vitest";
import { format, translate } from "./i18n";

describe("i18n", () => {
  it("translates a known key in both languages", () => {
    expect(translate("en", "hosts.title")).toBe("Hosts");
    expect(translate("th", "hosts.title")).toBe("โฮสต์");
  });

  it("falls back to English when a Thai string is missing", () => {
    // Every key added to `en` should eventually get a `th` entry, but the
    // fallback chain (th -> en -> key) must never surface as the raw key for
    // a key that does exist in English.
    expect(translate("th", "a.cancel")).not.toBe("a.cancel");
  });

  it("falls back to the raw key when totally unknown", () => {
    expect(translate("en", "nonexistent.key")).toBe("nonexistent.key");
    expect(translate("th", "nonexistent.key")).toBe("nonexistent.key");
  });

  it("formats placeholders", () => {
    expect(format("{client} wants to run \"{tool}\" on {host}", {
      client: "Claude Code",
      tool: "exec",
      host: "web",
    })).toBe('Claude Code wants to run "exec" on web');
  });

  it("leaves unknown placeholders untouched", () => {
    expect(format("hello {name}", {})).toBe("hello {name}");
  });
});
