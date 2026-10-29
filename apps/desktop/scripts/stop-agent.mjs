// Stops a running OurShell agent before `tauri dev` / `tauri build`.
//
// The agent outlives the desktop window on purpose, but on Windows a running
// .exe can't be overwritten, so the next build fails with "Access is denied"
// when tauri-build copies the sidecar to src-tauri/target/debug/ourshell.exe.
// Asking the agent to shut down (it locks the vault) frees the file.
import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const exe = process.platform === "win32" ? "ourshell.exe" : "ourshell";
const candidates = [
  join(here, "..", "src-tauri", "target", "debug", exe),
  join(here, "..", "..", "..", "target", "debug", exe),
  join(here, "..", "..", "..", "target", "release", exe),
];

const bin = candidates.find((p) => existsSync(p)) ?? exe; // fall back to PATH
try {
  const out = execFileSync(bin, ["vault", "stop-agent"], { timeout: 5000, encoding: "utf8" });
  if (/stopped/i.test(out)) console.log("[stop-agent] stopped the running OurShell agent");
} catch {
  // No binary yet, or no agent running: nothing to do.
}
