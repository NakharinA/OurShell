// Runs before `tauri dev` / `tauri build`: stops a running agent (its .exe is
// locked on Windows), rebuilds the `ourshell` agent and the `ourshell-mcp`
// server from the workspace, and copies them in as Tauri sidecars. Without this the app keeps launching a
// stale agent that lacks recent fixes.
import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const release = process.argv.includes("--release");
// Tauri sets this for before*Command hooks (e.g. `tauri build --target aarch64-pc-windows-msvc`),
// so the agent is built for the same architecture as the app bundle.
const target = process.env.TAURI_ENV_TARGET_TRIPLE || "";
const targetArgs = target ? ["--target", target] : [];
const run = (cmd, args, opts = {}) => execFileSync(cmd, args, { stdio: "inherit", ...opts });

run(process.execPath, [join(here, "stop-agent.mjs")]);
run("cargo", ["build", "-p", "ourshell", "-p", "ourshell-mcp", ...(release ? ["--release"] : []), ...targetArgs], {
  cwd: join(here, "..", "..", ".."),
});
run(process.execPath, [join(here, "copy-sidecar.mjs"), ...(release ? ["--release"] : []), ...targetArgs]);
