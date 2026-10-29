# Setting up the OurShell MCP server

The OurShell MCP server (`ourshell-mcp`) lets an AI client (Claude Desktop, Claude
Code, and similar) run commands and move files on SSH hosts you control — without
ever handing the AI a password or private key. It runs over stdio, holds no keys of
its own, and forwards every call to the local `ourshell` agent, which enforces your
policy, asks you to approve the call, runs it, and writes it to the audit log. See
`docs/agent-protocol.md` for the wire protocol and the "MCP design" section of the
project plan for the overall architecture.

## Install

```sh
cargo install --path crates/cli
cargo install --path crates/mcp
```

This installs two binaries: `ourshell` (the CLI and the agent daemon) and
`ourshell-mcp` (the MCP server). `ourshell-mcp` starts the agent for you if one
isn't already running, so you only need `ourshell` on your `PATH`.

## Mark hosts AI-allowed

By default no host is visible to AI tools. A host must be explicitly opted in:

```sh
ourshell host edit <name> --ai-allowed true
```

Optionally, run AI commands as a restricted user instead of whatever user you
normally log in as (recommended):

```sh
ourshell host edit <name> --ai-allowed true --ai-user ai
```

Only hosts marked this way are ever returned by the `list_hosts` tool, and every
other tool refuses any host that isn't on that list — dangerous mode (below) never
widens this set.

## Claude Code

```sh
claude mcp add ourshell -- ourshell-mcp
```

Claude Code will launch `ourshell-mcp` over stdio and discover the ten tools
(`list_hosts`, `run_command`, `list_dir`, `read_file`, `write_file`, `k8s_get`,
`k8s_describe`, `k8s_logs`, `k8s_events`, `system_status`) automatically.

If `/mcp` shows the server as failed (`CONNECTION_CLOSED`), Claude Code couldn't
start `ourshell-mcp`; usually it isn't on the `PATH` Claude Code sees. Either
install it (`cargo install --path crates/mcp`, which puts it in `~/.cargo/bin`)
or register it with an absolute path:

```sh
# Windows (a dev build)
claude mcp remove ourshell
claude mcp add ourshell -- C:\path\to\Ourshell\target\debug\ourshell-mcp.exe
# macOS / Linux
claude mcp add ourshell -- "$HOME/.cargo/bin/ourshell-mcp"
```

The MCP server and the desktop app must reach the same agent so that approvals
show up in the app: leave `OURSHELL_AGENT_SOCKET` unset (the default) for both.

As soon as Claude Code starts the MCP server, its session appears in the desktop
app under **MCP & AI → AI Access** (and in `ourshell ai status`), before the AI
has called any tool, so you can enable dangerous mode for it up front. Each
Claude Code session gets a new session id; a session that `/mcp` reconnects is a
new one, so dangerous mode has to be enabled again.

## Claude Desktop

Add an entry to `claude_desktop_config.json` and restart Claude Desktop. The file
lives at:

- **Windows:** `%APPDATA%\Claude\claude_desktop_config.json`
- **macOS:** `~/Library/Application Support/Claude/claude_desktop_config.json`
- **Linux:** `~/.config/Claude/claude_desktop_config.json`

```json
{
  "mcpServers": {
    "ourshell": {
      "command": "ourshell-mcp"
    }
  }
}
```

If `ourshell-mcp` isn't on the `PATH` that Claude Desktop sees (common on Windows
and macOS, since GUI apps often don't inherit your shell's `PATH`), use the full
install path instead:

```json
{
  "mcpServers": {
    "ourshell": {
      "command": "C:\\Users\\you\\.cargo\\bin\\ourshell-mcp.exe"
    }
  }
}
```

```json
{
  "mcpServers": {
    "ourshell": {
      "command": "/Users/you/.cargo/bin/ourshell-mcp"
    }
  }
}
```

```json
{
  "mcpServers": {
    "ourshell": {
      "command": "/home/you/.cargo/bin/ourshell-mcp"
    }
  }
}
```

(Windows, macOS, Linux respectively — adjust the path to wherever `cargo install`
put the binary, typically `~/.cargo/bin`.)

## Approving calls

With no further setup, every tool call pauses until a human approves it:

- If the OurShell desktop app is running, you'll see an approval prompt there.
- Otherwise, run `ourshell ai watch` in a terminal to see pending approvals as they
  arrive and approve or reject them from the command line.

Each pending approval shows the AI session, the host, the tool, and a short detail
(the command, or the path being read/written) so you can see exactly what you're
approving before you do.

## Read-only tools

Five tools never run an AI-supplied command at all: `k8s_get`, `k8s_describe`, `k8s_logs`,
`k8s_events` and `system_status`. For these, the AI only supplies a handful of strictly
validated arguments (a resource kind, a namespace, a name, ...), and the agent -- not the AI
-- builds the exact `kubectl`/status command from them. There is no way to pass a raw
command, an output-format flag, or anything beyond those arguments.

They can never show you configuration or secret *contents*: `configmaps` may be listed by
`k8s_get` (name and age only), but `k8s_describe` refuses `configmaps` outright, because
`describe` would print the object's data in full. Kubernetes `secret`s are not reachable
through any of these tools, in any case or spelling.

By default these tools still pause for your approval, exactly like every other tool. If you
want them to run immediately for a specific host -- say, so an AI assistant can check pod
status or tail logs without interrupting you every time -- turn on **Allow read-only AI tools
without approval** for that host (in the host's **AI access** section in the desktop app, or
`ourshell host edit <name> --ai-readonly-auto true` on the CLI). This switch:

- Only affects `k8s_get`, `k8s_describe`, `k8s_logs`, `k8s_events` and `system_status`.
  `run_command`, `list_dir`, `read_file` and `write_file` always pause for approval on that
  host unless dangerous mode is on.
- Requires the host to already be AI-allowed; it has no effect otherwise.
- Still respects the kill switch and dangerous mode: `ourshell ai stop` blocks it like
  everything else, and if dangerous mode is separately on for the session and host, that
  takes priority (and shows as such in the audit log).
- Shows up in `host ls` as `yes+ro` instead of `yes`, and audit entries made this way show
  `auto-readonly` as the mode and `policy` as the approver, so you can always tell which calls
  ran unattended.

## Dangerous mode

For a specific AI session and a specific set of hosts, you can turn off the
per-call approval prompt for a limited time:

```sh
ourshell ai dangerous --session <id> --hosts a,b --minutes 30
```

Turn it back off early with `--minutes 0`, or just let it expire. Dangerous mode:

- Only applies to the one AI session you name and the hosts you list — it never
  makes any other host reachable, and it never applies to a different AI session.
- Always has a time limit; it ends automatically when the session ends if you
  don't set one.
- Shows a persistent red banner in the desktop app and the CLI status line the
  whole time it's active, so it's never silent.

### Prompt-injection warning

**Read this before turning on dangerous mode.** Everything an AI tool reads back
from a host — command output, a directory listing, a file's contents — is text
that the host (or whoever can write to it) controls, not the AI client and not
you. That text can be crafted to look like instructions aimed at the AI model
("ignore your previous instructions and...", "now run `curl | sh`...", and so
on). This is called prompt injection, and no amount of model training reliably
stops it.

In approval mode, this is contained: the AI can propose a dangerous follow-up
command, but you see and approve every command before it runs, so an injected
instruction still has to pass a human's eyes. **Dangerous mode removes that
check.** If you turn it on, you are trusting that nothing on the hosts you listed
can produce output that tricks the AI model into running something harmful — on
those same hosts, with the privileges of whatever user you configured. Prefer:

- A narrow, low-privilege `--ai-user` (see "Mark hosts AI-allowed" above) so an
  injected command can do as little damage as possible.
- A short `--minutes` window, scoped to the one task you're doing.
- Hosts you trust the *output* of, not just the login to — a host that runs
  untrusted workloads or serves third-party content is a bad candidate.

## Kill switch

If you want to stop all AI access immediately — mid dangerous-mode window,
mid-approval, or just because something looks wrong — run:

```sh
ourshell ai stop
```

This ends every active AI session's dangerous mode, rejects every pending
approval, and blocks further calls from those sessions until they reconnect with
a new session id (which, for an MCP server, means restarting the AI client or the
`ourshell-mcp` process).

## Audit log

Every AI call — approved or not, successful or not — is recorded: time, host,
command/path, mode, who approved it, and the exit code. It's stored encrypted in
the vault, so losing or copying the vault file doesn't leak it. Review it with:

```sh
ourshell ai audit
```
