# OurShell

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

## License

MIT
