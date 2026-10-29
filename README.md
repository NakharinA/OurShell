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

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Please report security problems privately, as described in [SECURITY.md](SECURITY.md).

## License

MIT
