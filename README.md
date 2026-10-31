<p align="center"><img src="assets/logo/ourshell.svg" width="96" height="96" alt="OurShell logo"></p>

# OurShell

[![CI](https://github.com/NakharinA/OurShell/actions/workflows/ci.yml/badge.svg)](https://github.com/NakharinA/OurShell/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A self-hostable, open-source, Termius-style SSH client written in Rust. It includes a CLI, a desktop app (Tauri 2), an MCP server that lets AI run commands on hosts you approve, and a zero-knowledge sync server.

Secrets are always encrypted on the client before they reach the server. See [the project plan](Open-Source%20SSH%20Client%20—%20Project%20Plan.md) and [`docs/`](docs/).

## Features

- **SSH and Telnet hosts**: interactive terminals with tabs and split panes, saved credentials and keys, jump hosts, agent forwarding. Telnet is for switches, routers and older devices (terminal only, unencrypted).
- **Host groups**: nested groups with rename, subgroups and move-to; created in the app or with `ourshell group`.
- **SFTP, port forwarding and snippets**: browse and transfer files, local/remote forwards, run saved commands on one or many hosts.
- **Encrypted vault**: everything lives in a local vault encrypted with your master password (Argon2id + XChaCha20-Poly1305), with a recovery phrase.
- **Sync across computers**: an optional self-hosted server that only ever stores ciphertext. Team vaults let you share hosts with other people.
- **Server backups**: the sync server backs itself up on a schedule, encrypted with your passphrase, to any S3-compatible storage (AWS S3, R2, B2, Wasabi, MinIO, Synology C2), and skips the upload when nothing changed.
- **AI with approval**: an MCP server lets AI assistants list and use hosts you opt in, with every action approved by you in the app or CLI.
- **Imports**: `~/.ssh/config` and Termius CSV.

## Download

Get the desktop app, or the `ourshell` CLI and `ourshell-mcp` server, from [GitHub Releases](https://github.com/NakharinA/OurShell/releases/latest):

| Platform | Desktop app | CLI + MCP |
|---|---|---|
| Windows (x64, ARM64) | `OurShell_<version>_x64-setup.exe` / `_arm64-setup.exe` | `ourshell-v<version>-<arch>-pc-windows-msvc.zip` |
| macOS (Apple Silicon) | `OurShell_<version>_aarch64.dmg` (see [Installing on macOS](#installing-on-macos)) | `ourshell-v<version>-aarch64-apple-darwin.tar.gz` |
| Linux (x64, ARM64) | `.AppImage` (updates itself), `.deb`, `.rpm` | `ourshell-v<version>-<arch>-unknown-linux-gnu.tar.gz` |

Or build the CLI from source with `cargo install --path crates/cli`.

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
ourshell vault init                      # create the encrypted vault; save the recovery phrase
ourshell identity add me -u alice        # prompts for the SSH password (optional)
ourshell key gen laptop                  # or: ourshell key import laptop ~/.ssh/id_ed25519
ourshell host add web alice@10.0.0.5 --identity me
ourshell host add sw1 10.0.0.9 --protocol telnet   # a telnet device
ourshell group add prod && ourshell host edit web --group prod
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

The installer starts the server (with its own PostgreSQL, or one you already have) and prints a link to finish setup in your browser:

- **Setup wizard** (`/setup`): checks the database, generates the server's master key (save it), sets who can sign up and an admin password.
- **Admin page** (`/admin`): status, settings, invite codes and users.
- **HTTPS**: optional built-in Let's Encrypt certificate, or put it behind your own reverse proxy.

Then, in the app, open **Account & sync**, enter the server address and sign up. The first account becomes the owner. Re-run the installer with `--upgrade` to update.

### Deploy the Docker image yourself

No script needed: the server is one public image for amd64 and arm64 that any Docker tool can deploy.

| Setting | Value |
|---|---|
| Docker image | `ghcr.io/nakharina/ourshell-server:latest` (or a version, e.g. `:0.8.0`) |
| Registry URL | `https://ghcr.io` (GitHub Container Registry) |
| Username | none: the image is public |
| Password | none: the image is public |
| Port | `8443` (HTTP); optional built-in HTTPS on `8444`, published as `3000` by default (forward public TCP 443 to it for Let's Encrypt) |
| Behind a reverse proxy / ingress | set `OURSHELL_BEHIND_PROXY=true` (and `PUBLIC_URL=https://...`), publish only `8443`; the proxy handles HTTPS ([setup](docs/self-hosting.md#behind-a-reverse-proxy-or-ingress)) |
| Volume | a named volume at `/data` (configuration and master key) |
| Database | PostgreSQL: set `DATABASE_URL`, or enter it in the setup wizard |

If a tool insists on credentials for a registry (some do when you add `ghcr.io` as a registry), use any GitHub username and a [personal access token](https://github.com/settings/tokens) with only the `read:packages` scope, never your GitHub password.

For Synology Container Manager, Portainer and similar, paste the ready-made compose file (server + PostgreSQL) from [Without the installer: paste a compose file](docs/self-hosting.md#without-the-installer-paste-a-compose-file). Then open the container's log for the setup code and go to `http://<server-address>:8443/setup`.

See [`docs/self-hosting.md`](docs/self-hosting.md) for all options, HTTPS, upgrades and backups.

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
