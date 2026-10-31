<p align="center"><img src="assets/logo/ourshell.svg" width="96" height="96" alt="OurShell logo"></p>

# OurShell

[![CI](https://github.com/NakharinA/OurShell/actions/workflows/ci.yml/badge.svg)](https://github.com/NakharinA/OurShell/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A self-hostable, open-source, Termius-style SSH client written in Rust. It includes a CLI, a desktop app (Tauri 2), an MCP server that lets AI run commands on hosts you approve, and a zero-knowledge sync server.

Secrets are always encrypted on the client before they reach the server. See [the project plan](Open-Source%20SSH%20Client%20—%20Project%20Plan.md) and [`docs/`](docs/).

## Layout

```
crates/core     SSH + vault + crypto
crates/proto    shared wire types (blob format, agent JSON-RPC, sync API)
crates/cli      `ourshell` CLI and local agent daemon
crates/mcp      `ourshell-mcp` MCP server (stdio)
server          `ourshell-server` sync server (axum + PostgreSQL)
apps/desktop    Tauri 2 desktop app
docs            threat model, protocol specs
```

## Quick start (CLI)

```sh
cargo install --path crates/cli          # installs `ourshell`
ourshell vault init                      # create the encrypted vault; save the recovery phrase
ourshell identity add me -u alice        # prompts for the SSH password (optional)
ourshell key gen laptop                  # or: ourshell key import laptop ~/.ssh/id_ed25519
ourshell host add web alice@10.0.0.5 --identity me
ourshell connect web                     # interactive shell
ourshell exec web uptime                 # one command
ourshell sftp put web ./file.txt /tmp/file.txt
ourshell forward local web 8080 127.0.0.1:80
ourshell import ssh-config               # import ~/.ssh/config
ourshell vault lock
```

The CLI starts a local agent (`ourshell agent`) on demand. It holds the unlocked
vault, auto-locks after 15 idle minutes, and exits when idle and locked. The
desktop app and the MCP server talk to the same agent.

## Self-host the sync server

On a Linux server, a Synology NAS or a Mac with Docker:

```sh
curl -fsSL https://github.com/NakharinA/OurShell/releases/latest/download/install-server.sh | sh
```

On Windows with Docker Desktop (PowerShell):

```powershell
irm https://github.com/NakharinA/OurShell/releases/latest/download/install-server.ps1 | iex
```

The installer starts the server (with its own PostgreSQL, or one you already have) and prints a link to a setup wizard in your browser. Re-run it to upgrade. See [`docs/self-hosting.md`](docs/self-hosting.md) for options, backups and running it by hand.

## Installing on macOS

Release builds aren't notarized by Apple yet, so the first launch is blocked:

1. Open the DMG and drag **OurShell** to Applications.
2. Open it once; macOS says it "could not verify" OurShell. Click **Done**.
3. Go to **System Settings → Privacy & Security**, scroll down, click **Open Anyway** next to OurShell, and confirm.

If macOS instead says OurShell **"is damaged and can't be opened"** (builds from v0.6.0 and earlier), the app is fine; it's just unsigned. Clear the download quarantine flag and open it normally:

```sh
xattr -dr com.apple.quarantine /Applications/OurShell.app
```

## Updating

Updates only ever come from [GitHub Releases](https://github.com/NakharinA/OurShell/releases) on this repository.

- **Desktop app**: signed release builds check for updates on startup, every 6 hours, and via "Check for updates" in the sidebar's profile menu. When one is found, a banner offers "Install and restart" (downloads, verifies the signature, and relaunches). See [`docs/release-signing.md`](docs/release-signing.md) for how releases are signed and published.
- **CLI / MCP server**: run `ourshell update` to download, verify (SHA-256) and install the latest release in place, replacing the running `ourshell` (and `ourshell-mcp`, if present next to it). `ourshell update --check` only reports whether a newer version exists (exit code `10` if so, `0` otherwise -- handy in scripts) without installing anything.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Please report security problems privately, as described in [SECURITY.md](SECURITY.md).

## License

MIT
