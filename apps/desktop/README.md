# OurShell desktop app

Tauri 2 + Svelte 5 shell over the local `ourshell agent`. **This app never does
crypto or SSH itself** — it connects to the agent's local socket (a Windows
named pipe or a Unix domain socket) and forwards JSON-RPC 2.0 requests over it,
per [`docs/agent-protocol.md`](../../docs/agent-protocol.md) and the types in
[`crates/proto`](../../crates/proto). See `src-tauri/src/agent.rs` for the
connection/reconnect logic and `src/lib/rpc.ts` for the typed client the UI
calls.

```
apps/desktop/
  src/                    Svelte 5 + TypeScript frontend (Vite)
    lib/rpc.ts            typed JSON-RPC client (mirrors crates/proto)
    lib/types.ts          TS mirror of crates/proto/src/{rpc,agent}.rs
    lib/stores/           svelte stores: vault, items, ai, terminal, events, ui, theme
    lib/components/       unlock/create-vault, sidebar, host editor, terminal,
                           SFTP browser, port forwards, AI panel, ...
  src-tauri/              Tauri 2 Rust shell (its own Cargo workspace, not a
                           member of the root one)
    src/agent.rs          connects to the agent socket, spawns it if needed,
                           relays requests/events
    src/commands.rs       `rpc_call(method, params)` Tauri command
    binaries/             sidecar binaries land here (gitignored)
  scripts/copy-sidecar.mjs
```

## Dev workflow

```sh
cd apps/desktop
npm install

# Build the CLI/agent binary and stage it as the Tauri sidecar once (and
# again whenever crates/core or crates/cli change):
cargo build -p ourshell                 # from the repo root
node scripts/copy-sidecar.mjs           # copies target/debug/ourshell(.exe)
                                         # into src-tauri/binaries/

npm run tauri dev                       # launches the app with hot reload
```

On Windows, if `corepack` complains about the root `package.json`'s
`packageManager` field (it declares yarn, this app uses npm), run with
`COREPACK_ENABLE_STRICT=0` set, or always run npm commands from inside
`apps/desktop` as shown above.

If the agent socket isn't reachable when the app starts, `src-tauri/src/agent.rs`
tries, in order: the bundled sidecar, `ourshell` on `PATH`, then
`target/debug/ourshell(.exe)` under the repo root (a dev-only fallback so you
don't have to copy the sidecar just to iterate on the UI). It retries for a
few seconds before giving up, and keeps reconnecting (re-spawning if needed)
for the life of the app; `agent-status` events tell the UI what's going on.

## The sidecar

Tauri's `externalBin` mechanism (`tauri.conf.json`'s
`bundle.externalBin: ["binaries/ourshell"]`) requires a file named
`binaries/ourshell-<target-triple>(.exe)` to exist at build time, which is
what `scripts/copy-sidecar.mjs` produces from whatever's at
`target/debug/ourshell(.exe)` (or `target/release/...` with `--release`) in
the root Cargo workspace:

```sh
cargo build --release -p ourshell
node scripts/copy-sidecar.mjs --release
```

`src-tauri/binaries/` is gitignored — every machine (and CI) regenerates it
from a fresh `cargo build`.

## Verifying a change

```sh
npm run check      # svelte-check + tsc, should report 0 errors
npm run build      # vite build
npm run test       # vitest: base64 helpers + event routing in rpc.ts

cd src-tauri
cargo clippy -- -D warnings
cargo build
```

To prove the whole Tauri app builds end-to-end without creating a full
installer:

```sh
node scripts/copy-sidecar.mjs      # needs target/debug/ourshell(.exe) built first
npx tauri build --debug --no-bundle
```

## Building installers

```sh
cargo build --release -p ourshell
node scripts/copy-sidecar.mjs --release

npx tauri build --bundles nsis,msi   # Windows
npx tauri build --bundles dmg        # macOS
npx tauri build --bundles deb,appimage  # Linux
```

Linux builds need the WebKitGTK/appindicator dev packages Tauri links
against, e.g. on Ubuntu:

```sh
sudo apt-get install -y libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

`.github/workflows/desktop.yml` builds the bundle on Windows, macOS and Linux
on every push/PR that touches `apps/desktop/` or `crates/`.

## Auto-update

The Tauri updater plugin is **not** wired up yet — that's Phase 6
(`cargo-dist`/signed releases) in the project plan. Leaving this as a TODO
rather than a half-configured plugin: don't add `tauri-plugin-updater` until
there's a release pipeline that actually signs and publishes update manifests.

## Protocol assumptions this app makes

The agent is built in parallel by someone else; the desktop app is coded
against `docs/agent-protocol.md` and `crates/proto`, not an implementation.
Things the agent needs to satisfy for this UI to work (see "Protocol
assumptions" in the implementation notes / PR description for the full list):

- The handshake (`hello`) must succeed before any other call on a connection,
  and the agent must tolerate the desktop client reconnecting at any time
  (e.g. after the agent restarts).
- `pty.open` on `-32006` (`HOST_KEY_UNKNOWN`) must set `error.data` to the
  fingerprint string to show the user; retrying with the same params plus
  `accept_host_key` set to that fingerprint must then succeed.
- `item.put` must accept `id: null` to create and an existing id to update,
  returning the stored `ItemRecord`.
- `ai.status`'s `pending` must match what already went out as
  `approval.request` events (the UI reconciles both).

> **Note:** the agent keeps running after you close the app, by design. On Windows a running
> `.exe` can't be overwritten, so `npm run tauri dev` / `tauri build` first run
> `scripts/stop-agent.mjs`, which asks any running agent to shut down (this locks the vault).
> If you still see `Access is denied` from `tauri-build`, run `ourshell vault stop-agent`.
