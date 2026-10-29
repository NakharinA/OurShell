#!/usr/bin/env node
// Fails if the version recorded in any of the project's manifests disagrees
// with the workspace version (the single source of truth, in the root
// Cargo.toml's [workspace.package]). Run by CI (.github/workflows/ci.yml) on
// every push/PR, and by the release workflows to confirm the pushed tag
// matches the version actually being released.
//
// Usage:
//   node scripts/check-version.mjs            # just checks all files agree
//   node scripts/check-version.mjs --tag v1.2.3   # also checks the tag matches

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, "..");

const FILES = {
  "Cargo.toml (workspace)": {
    path: join(repoRoot, "Cargo.toml"),
    read: (text) => matchOrThrow(text, /\[workspace\.package\][^[]*?\bversion\s*=\s*"([^"]+)"/s, "Cargo.toml"),
  },
  "apps/desktop/src-tauri/Cargo.toml": {
    path: join(repoRoot, "apps/desktop/src-tauri/Cargo.toml"),
    read: (text) => matchOrThrow(text, /\[package\][^[]*?\bversion\s*=\s*"([^"]+)"/s, "src-tauri/Cargo.toml"),
  },
  "apps/desktop/src-tauri/tauri.conf.json": {
    path: join(repoRoot, "apps/desktop/src-tauri/tauri.conf.json"),
    read: (text) => JSON.parse(text).version,
  },
  "apps/desktop/package.json": {
    path: join(repoRoot, "apps/desktop/package.json"),
    read: (text) => JSON.parse(text).version,
  },
};

function matchOrThrow(text, re, label) {
  const m = text.match(re);
  if (!m) throw new Error(`could not find a version in ${label}`);
  return m[1];
}

function readVersion(entry) {
  const text = readFileSync(entry.path, "utf8");
  return entry.read(text);
}

function main() {
  const versions = {};
  for (const [label, entry] of Object.entries(FILES)) {
    try {
      versions[label] = readVersion(entry);
    } catch (e) {
      console.error(`error: failed to read version from ${label}: ${e.message}`);
      process.exit(1);
    }
  }

  const labels = Object.keys(versions);
  const canonical = versions[labels[0]];
  let ok = true;

  console.log("Versions found:");
  for (const label of labels) {
    const v = versions[label];
    const mark = v === canonical ? "" : "  <-- MISMATCH";
    console.log(`  ${v.padEnd(12)} ${label}${mark}`);
    if (v !== canonical) ok = false;
  }

  const tagArgIndex = process.argv.indexOf("--tag");
  if (tagArgIndex !== -1) {
    const tag = process.argv[tagArgIndex + 1];
    if (!tag) {
      console.error("error: --tag requires a value, e.g. --tag v1.2.3");
      process.exit(1);
    }
    const expected = `v${canonical}`;
    console.log(`\nTag:        ${tag}`);
    console.log(`Expected:   ${expected}`);
    if (tag !== expected) {
      console.error(
        `\nerror: pushed tag "${tag}" does not match the workspace version "${canonical}" ` +
          `(expected "${expected}"). Did you forget to run scripts/bump-version.mjs, or tag the ` +
          "wrong commit?",
      );
      process.exit(1);
    }
  }

  if (!ok) {
    console.error(
      "\nerror: version mismatch across manifests. Run `node scripts/bump-version.mjs <x.y.z>` " +
        "to fix all of them at once.",
    );
    process.exit(1);
  }

  console.log("\nAll versions agree.");
}

main();
