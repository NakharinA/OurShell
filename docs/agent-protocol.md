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
| `ai.k8s_get` / `ai.k8s_describe` / `ai.k8s_logs` / `ai.k8s_events` / `ai.system_status` | Typed, read-only AI calls: the agent builds the remote `kubectl`/status command itself from validated arguments (see "Typed read-only AI tools" below), never from client text |
| `ai.approvals` / `ai.decide` | Pending approvals and the user's decision |
| `ai.dangerous` / `ai.stop` / `ai.audit` | Dangerous-mode toggle, kill switch, audit log |
| `events.subscribe` | Receive `vault.locked`, `approval.request` and `ai.mode` events |

## Typed read-only AI tools

`ai.k8s_get`, `ai.k8s_describe`, `ai.k8s_logs`, `ai.k8s_events` and `ai.system_status` never
carry a command: the AI passes a handful of strictly validated parameters (`host`, `kind`,
`namespace`, `name`, ...) and the agent assembles the exact remote command itself --
`kubectl get <kind> -n <namespace> [name] -o wide`, `kubectl describe <kind> <name> -n
<namespace>`, `kubectl logs <pod> -n <namespace> [-c <container>] --tail=<N> [--since=<D>]`,
`kubectl get events -n <namespace> --sort-by=.lastTimestamp`, or the fixed `uptime; df -h;
free -m`. There is no way to pass a raw command, an output-format flag, or anything else
through these methods.

`kind` is one of a fixed allowlist with aliases (`po`/`pod`/`pods`, `deploy`/`deployment`/
`deployments`, `sts`, `ds`, `rs`, `svc`/`service`, `ing`/`ingress`, `no`/`node`, `ns`/
`namespace`, `job`, `cj`/`cronjob`, `pvc`, `pv`, `ev`/`event`, `ep`/`endpoint`, `cm`/
`configmap`); anything else -- including every spelling of `secret`/`secrets` -- is rejected.
`configmaps` may be listed by `ai.k8s_get` (name/age only) but never described by
`ai.k8s_describe`, since `describe` prints the object's data in full. Names, namespaces, pods
and containers must be valid Kubernetes DNS names; `tail` is capped at 500; `since` is capped
at 7 days. A bad parameter fails with `-32602` (invalid params) before any connection is made.

These methods still go through the same host (`ai_allowed`), dangerous-mode and kill-switch
checks as every other `ai.*` call, are audited and redacted the same way, and use the same
`for_ai` SSH path (so `ai_user` still applies). The one difference: a host can separately opt
in (`ai_readonly_auto`) to skipping the human-approval wait for *just these tools* -- see
`docs/mcp-setup.md`.

## Errors

Standard JSON-RPC errors, plus: `-32001` vault locked, `-32002` not found, `-32003` denied by policy, `-32004` approval rejected, `-32005` approval timed out.
