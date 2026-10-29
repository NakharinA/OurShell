// Runs before `tauri dev` / `tauri build`: stops a running agent (its .exe is
// locked on Windows), rebuilds the `ourshell` agent from the workspace, and
// copies it in as the Tauri sidecar. Without this the app keeps launching a
// stale agent that lacks recent fixes.
import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const release = process.argv.includes("--release");
const run = (cmd, args, opts = {}) => execFileSync(cmd, args, { stdio: "inherit", ...opts });

run(process.execPath, [join(here, "stop-agent.mjs")]);
run("cargo", ["build", "-p", "ourshell", ...(release ? ["--release"] : [])], {
  cwd: join(here, "..", "..", ".."),
});
run(process.execPath, [join(here, "copy-sidecar.mjs"), ...(release ? ["--release"] : [])]);
