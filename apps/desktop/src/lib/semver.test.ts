import { describe, expect, it } from "vitest";
import { compareSemver, isNewerVersion, parseSemver } from "./semver";

describe("parseSemver", () => {
  it("parses a plain version", () => {
    expect(parseSemver("1.2.3")).toEqual({ major: 1, minor: 2, patch: 3, prerelease: [] });
  });

  it("strips a leading v (git tag style)", () => {
    expect(parseSemver("v0.6.0")).toEqual({ major: 0, minor: 6, patch: 0, prerelease: [] });
  });

  it("parses prerelease identifiers", () => {
    expect(parseSemver("1.0.0-rc.1")).toEqual({ major: 1, minor: 0, patch: 0, prerelease: ["rc", "1"] });
  });

  it("ignores build metadata", () => {
    expect(parseSemver("1.0.0+build.5")).toEqual({ major: 1, minor: 0, patch: 0, prerelease: [] });
  });

  it("returns null for invalid input", () => {
    expect(parseSemver("not-a-version")).toBeNull();
    expect(parseSemver("1.2")).toBeNull();
    expect(parseSemver("")).toBeNull();
  });
});

describe("compareSemver", () => {
  it("orders by major, then minor, then patch", () => {
    expect(compareSemver(parseSemver("2.0.0")!, parseSemver("1.9.9")!)).toBeGreaterThan(0);
    expect(compareSemver(parseSemver("1.2.0")!, parseSemver("1.3.0")!)).toBeLessThan(0);
    expect(compareSemver(parseSemver("1.2.3")!, parseSemver("1.2.4")!)).toBeLessThan(0);
    expect(compareSemver(parseSemver("1.2.3")!, parseSemver("1.2.3")!)).toBe(0);
  });

  it("ranks a prerelease below its final release", () => {
    expect(compareSemver(parseSemver("1.0.0-alpha")!, parseSemver("1.0.0")!)).toBeLessThan(0);
    expect(compareSemver(parseSemver("1.0.0")!, parseSemver("1.0.0-alpha")!)).toBeGreaterThan(0);
  });

  it("compares prerelease identifiers numerically then lexically", () => {
    // 1.0.0-alpha < 1.0.0-alpha.1 < 1.0.0-alpha.beta < 1.0.0-beta < 1.0.0-beta.2 < 1.0.0-beta.11 < 1.0.0-rc.1 < 1.0.0
    const chain = [
      "1.0.0-alpha",
      "1.0.0-alpha.1",
      "1.0.0-alpha.beta",
      "1.0.0-beta",
      "1.0.0-beta.2",
      "1.0.0-beta.11",
      "1.0.0-rc.1",
      "1.0.0",
    ].map((v) => parseSemver(v)!);
    for (let i = 0; i < chain.length - 1; i++) {
      expect(compareSemver(chain[i], chain[i + 1])).toBeLessThan(0);
    }
  });
});

describe("isNewerVersion", () => {
  it("reports a higher version as newer", () => {
    expect(isNewerVersion("0.6.0", "0.7.0")).toBe(true);
    expect(isNewerVersion("0.6.0", "v0.7.0")).toBe(true);
  });

  it("reports an equal or lower version as not newer", () => {
    expect(isNewerVersion("0.6.0", "0.6.0")).toBe(false);
    expect(isNewerVersion("0.6.0", "0.5.0")).toBe(false);
  });

  it("is prerelease-aware", () => {
    expect(isNewerVersion("1.0.0-rc.1", "1.0.0")).toBe(true);
    expect(isNewerVersion("1.0.0", "1.0.0-rc.1")).toBe(false);
  });

  it("treats invalid input as not newer", () => {
    expect(isNewerVersion("0.6.0", "not-a-version")).toBe(false);
    expect(isNewerVersion("garbage", "0.7.0")).toBe(false);
  });
});
