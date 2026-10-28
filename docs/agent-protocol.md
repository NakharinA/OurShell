# Local agent protocol (v1)

The agent (`ourshell agent`) holds the unlocked vault. The CLI, the desktop app and the MCP server talk to it over a local socket:

- Windows: named pipe `\\.\pipe\ourshell-agent-<username>`
- macOS/Linux: Unix socket `$XDG_RUNTIME_DIR/ourshell/agent.sock` (fallback `~/.ourshell/agent.sock`), mode `0600`

## Framing

JSON-RPC 2.0, one JSON object per line (UTF-8, `\n` terminated). Requests carry an `id`; notifications (agent → client events) do not. Binary data (PTY bytes, file contents) is base64.

## Handshake

The first request on every connection must be `hello`:

```json
{"jsonrpc":"2.0","id":1,"method":"hello","params":{"protocol":1,"client":"cli","ai_session":null}}
```

The agent replies with its protocol version and rejects a different major version. `client` is one of `cli`, `desktop` or `mcp`. MCP connections must pass `ai_session` (a random id per MCP server process); only those connections go through the AI policy.

## Methods

| Method | Purpose |
| --- | --- |
| `vault.status` / `vault.unlock` / `vault.lock` | Lock state; unlock with the password |
| `host.list` / `host.get` / `host.put` / `host.delete` | Host CRUD |
| `identity.*`, `key.*`, `snippet.*`, `group.*` | Same shape as `host.*` |
| `key.generate` / `key.import` | Key tools |
| `pty.open` / `pty.write` / `pty.resize` / `pty.close` | Interactive sessions; output arrives as `pty.data` / `pty.exit` events |
| `exec.run` | Run one command, return stdout, stderr and exit code |
| `sftp.list` / `sftp.read` / `sftp.write` | File access |
| `forward.open` / `forward.list` / `forward.close` | Port forwards |
| `ai.hosts` / `ai.run` / `ai.list_dir` / `ai.read_file` / `ai.write_file` | AI-facing calls, checked against the policy |
| `ai.approvals` / `ai.decide` | Pending approvals and the user's decision |
| `ai.dangerous` / `ai.stop` / `ai.audit` | Dangerous-mode toggle, kill switch, audit log |
| `events.subscribe` | Receive `vault.locked`, `approval.request` and `ai.mode` events |

## Errors

Standard JSON-RPC errors, plus: `-32001` vault locked, `-32002` not found, `-32003` denied by policy, `-32004` approval rejected, `-32005` approval timed out.
