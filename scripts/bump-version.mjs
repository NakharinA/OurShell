#!/usr/bin/env node
// Bumps OurShell's version everywhere it is recorded, so the workspace
// version (root Cargo.toml's [workspace.package]) stays the single source of
// truth: every crate inherits it via `version.workspace = true`, and this
// script keeps the three places that *can't* inherit it (the desktop app's
// own Cargo workspace, its tauri.conf.json, and its package.json) in sync.
//
// Usage:
//   node scripts/bump-version.mjs 0.7.0
//
// Refuses anything that isn't valid semver (optionally with a
// -prerelease/+build suffix). Run `node scripts/check-version.mjs`
// afterwards (or just trust this script - it rewrites every file the same
// way) to confirm everything agrees.

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, "..");

// https://semver.org/#is-there-a-suggested-regular-expression-regex-to-check-a-semver-string
const SEMVER_RE =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?(?:\+([0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?$/;

function usageAndExit(message) {
  if (message) console.error(`error: ${message}\n`);
  console.error("usage: node scripts/bump-version.mjs <x.y.z>");
  process.exit(1);
}

function replaceOrThrow(text, re, replacement, label) {
  if (!re.test(text)) {
    throw new Error(`pattern not found in ${label}; the file's format may have changed`);
  }
  return text.replace(re, replacement);
}

function bumpCargoToml(path, label, versionRe) {
  const text = readFileSync(path, "utf8");
  const updated = replaceOrThrow(text, versionRe, `$1"${version}"`, label);
  writeFileSync(path, updated);
  console.log(`updated ${label}`);
}

function bumpJson(path, label) {
  const text = readFileSync(path, "utf8");
  const data = JSON.parse(text);
  data.version = version;
  // Preserve trailing newline / 2-space indent style used across the repo's JSON files.
  writeFileSync(path, JSON.stringify(data, null, 2) + "\n");
  console.log(`updated ${label}`);
}

const version = process.argv[2];
if (!version) usageAndExit("missing version argument");
if (!SEMVER_RE.test(version)) {
  usageAndExit(`"${version}" is not a valid semver version (expected e.g. "1.2.3" or "1.2.3-rc.1")`);
}

try {
  // Root workspace Cargo.toml: `[workspace.package]` ... `version = "..."`.
  bumpCargoToml(
    join(repoRoot, "Cargo.toml"),
    "Cargo.toml",
    /(\[workspace\.package\][^[]*?\bversion\s*=\s*)"[^"]+"/s,
  );

  // The desktop app's src-tauri crate is its own Cargo workspace (excluded
  // from the root one, see root Cargo.toml's `exclude`), so it can't use
  // `version.workspace = true` and needs its own literal version kept in sync.
  bumpCargoToml(
    join(repoRoot, "apps/desktop/src-tauri/Cargo.toml"),
    "apps/desktop/src-tauri/Cargo.toml",
    /(\[package\][^[]*?\bversion\s*=\s*)"[^"]+"/s,
  );

  bumpJson(join(repoRoot, "apps/desktop/src-tauri/tauri.conf.json"), "apps/desktop/src-tauri/tauri.conf.json");
  bumpJson(join(repoRoot, "apps/desktop/package.json"), "apps/desktop/package.json");
} catch (e) {
  console.error(`error: ${e.message}`);
  process.exit(1);
}

console.log(`\nOurShell is now at v${version} everywhere. Next steps:\n`);
console.log("  git add -A");
console.log(`  git commit -m "chore: bump version to ${version}"`);
console.log(`  git tag -a v${version} -m "OurShell v${version}"`);
console.log("  git push --follow-tags");
console.log(
  "\nPushing the tag triggers .github/workflows/release.yml, which builds the CLI, MCP server and",
);
console.log("desktop app into one draft GitHub release. Review it, then publish to go live.");
