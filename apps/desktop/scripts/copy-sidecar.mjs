#!/usr/bin/env node
// Copies the built `ourshell` binary (the CLI + local agent, see crates/cli) into
// `src-tauri/binaries/ourshell-<target-triple>(.exe)`, the layout Tauri's
// `externalBin` sidecar mechanism requires (see tauri.conf.json's
// `bundle.externalBin: ["binaries/ourshell"]`).
//
// Usage:
//   node scripts/copy-sidecar.mjs            # copies target/debug build
//   node scripts/copy-sidecar.mjs --release  # copies target/release build
//   node scripts/copy-sidecar.mjs --release --target aarch64-pc-windows-msvc
//                                            # copies target/<triple>/release (cross build)
//
// Run this after `cargo build [--release] -p ourshell` and before
// `npm run tauri dev` / `npx tauri build`, since Tauri needs the sidecar file to
// exist on disk at build/dev time (see README.md).

import { existsSync, mkdirSync, copyFileSync, chmodSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
// apps/desktop/scripts -> repo root
const repoRoot = join(scriptDir, "..", "..", "..");
const desktopDir = join(scriptDir, "..");

const release = process.argv.includes("--release");
const profile = release ? "release" : "debug";
const targetArg = process.argv.indexOf("--target");
const requestedTarget = targetArg >= 0 ? process.argv[targetArg + 1] : undefined;

function hostTriple() {
  const out = execFileSync("rustc", ["-vV"], { encoding: "utf8" });
  const line = out.split("\n").find((l) => l.startsWith("host: "));
  if (!line) {
    throw new Error("could not determine host target triple from `rustc -vV`");
  }
  return line.slice("host: ".length).trim();
}

function main() {
  const host = hostTriple();
  const triple = requestedTarget || host;
  const exe = triple.includes("windows") ? ".exe" : "";
  // `cargo build --target X` writes to target/X/<profile>, even when X is the host.
  const src = requestedTarget
    ? join(repoRoot, "target", triple, profile, `ourshell${exe}`)
    : join(repoRoot, "target", profile, `ourshell${exe}`);
  if (!existsSync(src)) {
    console.error(
      `error: ${src} does not exist.\n` +
        `Build it first, e.g.: cargo build ${release ? "--release " : ""}` +
        `${requestedTarget ? `--target ${requestedTarget} ` : ""}-p ourshell`
    );
    process.exit(1);
  }

  const binariesDir = join(desktopDir, "src-tauri", "binaries");
  mkdirSync(binariesDir, { recursive: true });
  const dest = join(binariesDir, `ourshell-${triple}${exe}`);
  copyFileSync(src, dest);
  if (!triple.includes("windows")) {
    chmodSync(dest, 0o755);
  }
  console.log(`copied ${src} -> ${dest}`);
}

main();
