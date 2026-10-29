import { describe, expect, it } from "vitest";
import { parseQuickConnect } from "./quickConnect";

describe("parseQuickConnect", () => {
  const hostNames = ["proxy.example.com", "staging-remote-dev", "web"];

  it("returns empty for blank input", () => {
    expect(parseQuickConnect("", hostNames)).toEqual({ type: "empty" });
    expect(parseQuickConnect("   ", hostNames)).toEqual({ type: "empty" });
  });

  it("matches an existing host name case-insensitively", () => {
    expect(parseQuickConnect("Proxy.Example.com", hostNames)).toEqual({
      type: "connect",
      hostName: "proxy.example.com",
    });
    expect(parseQuickConnect("web", hostNames)).toEqual({ type: "connect", hostName: "web" });
  });

  it("parses user@host into a prefilled new-host request", () => {
    expect(parseQuickConnect("root@10.0.0.5", hostNames)).toEqual({
      type: "new",
      username: "root",
      hostname: "10.0.0.5",
      port: null,
    });
  });

  it("parses user@host:port", () => {
    expect(parseQuickConnect("root@10.0.0.5:2222", hostNames)).toEqual({
      type: "new",
      username: "root",
      hostname: "10.0.0.5",
      port: 2222,
    });
  });

  it("parses a bare hostname with a dot (no existing match) as new", () => {
    expect(parseQuickConnect("new-box.internal", hostNames)).toEqual({
      type: "new",
      username: null,
      hostname: "new-box.internal",
      port: null,
    });
  });

  it("parses host:port with no user", () => {
    expect(parseQuickConnect("10.0.0.9:22", hostNames)).toEqual({
      type: "new",
      username: null,
      hostname: "10.0.0.9",
      port: 22,
    });
  });

  it("ignores an out-of-range port", () => {
    expect(parseQuickConnect("root@host:999999", hostNames)).toEqual({
      type: "new",
      username: "root",
      hostname: "host",
      port: null,
    });
  });

  it("treats a bare word with no @, : or . as a plain search miss", () => {
    expect(parseQuickConnect("nomatch", hostNames)).toEqual({ type: "none", query: "nomatch" });
  });
});
