# OurShell — Project Plan

Oct 28, 2026 · @pueyleng

## Overview

We are building a self-hostable, open-source SSH client inspired by Termius. It has a desktop app, a CLI, a sync server, and an MCP server that lets AI run commands on hosts the user has approved. Secrets are always encrypted on the client before they reach the server.

**Decisions so far**

| Topic | Decision |
| --- | --- |
| Team | Solo developer, hobby project, no deadline |
| Language | Rust for everything: core, CLI, desktop app, MCP server and sync server |
| Backend | Rust (axum), PostgreSQL |
| Platforms | Desktop only (Windows, macOS, Linux) with a Tauri 2 shell; no mobile for now |
| Hosting | Self-hosted, designed so a company can also deploy it for its staff; a local-only vault (no server) is also supported |
| Users | Personal vaults first; team vaults later |
| AI access | Human approval by default; an opt-in dangerous mode scoped to one AI session and the hosts the user picks |
| Import | `~/.ssh/config` import; Termius import if feasible |
| Encryption | Each client holds a private key; the server keeps public keys; sensitive data is encrypted before it is stored; login uses OPAQUE so the password never leaves the device |

**Release order**

1. **v0.1:** Rust core, encrypted local vault, CLI
2. **v0.2:** MCP server with approval and dangerous modes
3. **v0.3:** Desktop app
4. **v0.4:** Sync server and multi-device support
5. **Later:** Team vaults and company features such as SSO and admin controls

The project is named OurShell and released under the MIT license. It can be described as a Termius-style SSH client, without using the Termius name or branding.

## Architecture & tech stack

A single Rust core library holds the SSH, crypto and vault logic. The CLI, the desktop app and the MCP server are thin front-ends over that library, so the security-critical code is written once. The Rust sync server only ever stores ciphertext, and it shares types and crypto code with the clients.

&#91;embedded content: architecture · 3 front-ends, 1 local agent, 1 sync server\]

All three front-ends go through the local agent, so keys are unlocked in one place and every AI action passes its policy check.

| Component | Tech | Notes |
| --- | --- | --- |
| `core` crate | Rust, `tokio`, `russh`, `ssh-key` | SSH sessions, SFTP, port forwarding, jump hosts, known hosts |
| `crypto` module | `argon2`, `chacha20poly1305`, `x25519-dalek`, `ed25519-dalek` | Key derivation, encryption, key exchange, signatures |
| `vault` module | SQLite (`rusqlite`), encrypted rows | Local store of hosts, keys, passwords and snippets |
| `cli` crate | `clap`, `ratatui` (optional TUI) | First front-end; the easiest place to learn Rust |
| `mcp` crate | `rmcp` (official Rust MCP SDK) | Runs over stdio and talks to the local vault and agent |
| `desktop` app | Tauri 2, a web UI (Svelte or React), `xterm.js` | Bundles the agent as a sidecar and talks to it over JSON-RPC |
| `server` | Rust, axum, sqlx, opaque-ke, PostgreSQL | Accounts, devices, encrypted blob sync |
| Packaging | Docker Compose for the server; Tauri bundler for installers | Signed releases through GitHub Actions |

**Repo layout (monorepo)**

```
/crates/core      SSH + vault + crypto
/crates/cli
/crates/mcp
/apps/desktop     Tauri app
/server           Rust (axum) sync server
/docs             threat model, protocol spec
```

**Rust vs Go note:** You know Go better, so begin with the CLI. It is the smallest front-end and teaches you Rust's async model and ownership before you touch Tauri. Because the desktop app only talks to the agent over JSON-RPC, the UI shell does not depend on the core's language.

**Desktop app talks to the agent, not the core.** The desktop app ships the same binary as the CLI and starts `cli agent` as a Tauri sidecar. It sends JSON-RPC requests over the local socket and never parses CLI text output. That keeps the UI free of Rust and crypto code, and the CLI, desktop app and MCP server always share one vault session.

| Approach | Verdict | Why |
| --- | --- | --- |
| Desktop links the core crate directly | Not chosen | Duplicates session and vault state; MCP and desktop would hold separate unlocked vaults |
| Desktop runs CLI commands and parses their text | Avoid | Breaks on wording changes; no live terminal streams, events or approval prompts |
| Desktop talks to the agent over JSON-RPC | Chosen | One API for CLI, desktop and MCP; streaming PTY; easy to test |

## Encryption & key design

Every secret is encrypted on the client with keys the server never holds. The server stores only ciphertext and **public** keys. This follows your idea of a private key on the client and a public key on the server, and makes the server zero-knowledge: a stolen database or a malicious admin cannot read SSH passwords or private keys.

**Why the server must not hold a decrypting private key:** if the server could decrypt the data, anyone who breaks into the server could too. So the public key on the server is used only to *lock* data for a client. Only the client's private key can *unlock* it.

**Key hierarchy**

| Key | Type | Where it lives | Purpose |
| --- | --- | --- | --- |
| Master password | Secret the user remembers | User's head only | Starts everything; never sent anywhere |
| Master key | 256-bit, from Argon2id(password, salt) | Client memory only | Wraps the account private keys |
| OPAQUE login record | OPAQUE registration from the master password | Server stores the record; nothing password-derived is sent at login | Password-less login proof; the modern successor to SRP, which Termius uses |
| Account key pair | X25519 (encrypt) + Ed25519 (sign) | Public half: server, plain. Private half: server, wrapped by the master key | Unlocks the vault key; signs changes |
| Device key pair | Ed25519 | Private half: OS keychain. Public half: server | Device login and revocation |
| Vault key | Random 256-bit symmetric | Server, sealed to the account public key | Wraps the per-item keys |
| Item key | Random 256-bit per item | Server, wrapped by the vault key | Encrypts one host, credential or key with XChaCha20-Poly1305 |
| Recovery key | Random 256-bit, shown once as words | User prints or saves it | Second wrap of the vault key if the password is lost |

Per-item keys look like extra work. They pay off later: sharing one host with a teammate means re-wrapping one small key, not re-encrypting the whole vault.

**Saving a secret (for example an SSH password)**

1. The client creates a random item key and encrypts the item JSON with XChaCha20-Poly1305.
2. The client wraps the item key with the vault key.
3. The client signs the result with the account Ed25519 key.
4. The client sends `{id, type, version, ciphertext, wrapped_key, signature}` to the server.
5. The server checks the session and version, then stores the blob. It cannot read it.

**Adding a new device**

1. The user signs in with email and master password over OPAQUE. The client derives the master key locally; neither the password nor a hash of it crosses the network.
2. The server returns the wrapped account private key; the client unwraps it with the master key.
3. The new device generates its own device key pair and registers the public half.
4. The client opens the sealed vault key and downloads the item blobs.

**What is encrypted and what is not**

- Encrypted: hostnames, IPs, ports, usernames, passwords, private keys, passphrases, snippets, notes, labels and groups
- Plain (needed for sync): item id, item type, version number, timestamps, owner id, device ids

**Second layer on the server:** The database also uses envelope encryption at rest with a server master key from an environment variable or a KMS. This guards against stolen backups and disks only; the client-side layer is the real protection.

**Rules**

- Use well-known libraries only; never write your own crypto primitives.
- Unique random nonces (XChaCha20's 24-byte nonce makes this safe).
- Wipe keys from memory after use (`zeroize` crate).
- A forgotten master password with no recovery key means the data is gone. Tell users this clearly at signup.

**Lessons from Termius**

Our model matches Termius's: a key pair per account, a private key wrapped by the password, and vault keys sealed to members' public keys ([Termius encryption overview](https://docs.termius.com/security/encryption-overview)). We take three more ideas from it:

- **Password-less login:** Termius uses SRP; we use OPAQUE, its modern successor, so no password or password hash crosses the network.
- **Local-only vault:** a vault that never syncs, for users who want no server at all.
- **Sign-in separate from encryption:** Google or SSO can sign a user in, but a separate master password still unlocks the vault.

## MCP design

The AI never sees a password or private key; it can only ask a local agent to act on hosts the user has opted in. Every action waits for human approval unless the user turns on dangerous mode for a limited time.

**How it connects**

1. A local **agent daemon** (like `ssh-agent`) holds the unlocked vault in memory. The CLI, desktop app and MCP server all talk to it over a Unix socket or a Windows named pipe.
2. The MCP server runs over stdio inside the AI client (Claude Desktop, Claude Code, and so on). It has no keys of its own.
3. When the AI calls a tool, the MCP server asks the agent. The agent checks the policy, asks the user if needed, runs the command, and returns the output.

**Tools**

| Tool | What it does | Risk |
| --- | --- | --- |
| `list_hosts` | Names and tags of AI-allowed hosts; never credentials | Low |
| `run_command` | Runs one command on one host and returns stdout, stderr and exit code | High |
| `list_dir` / `read_file` | SFTP read access | Medium |
| `write_file` | SFTP upload or overwrite | High |
| `open_tunnel` (later) | Starts a port forward | High |

**Modes**

|  | Approval mode (default) | Dangerous mode (opt-in) |
| --- | --- | --- |
| Who approves | The user, for every call | Nobody; calls run immediately |
| How to turn on | Default | Explicit toggle for the current AI session and the hosts the user picks, with a warning screen |
| Time limit | None | Ends when the AI session ends; optional earlier time limit |
| Hosts | Only hosts marked "AI allowed" | Same; dangerous mode never widens which hosts are reachable |
| Visible signal | Approval prompt | Red banner in the desktop app and the CLI status line |

**Where the approval prompt appears:** a desktop notification or dialog when the app is running, otherwise a prompt in the CLI. MCP elicitation can be used when the AI client supports it.

**Safety rails that stay on in both modes**

- Per-host opt-in: hosts are hidden from the AI unless marked "AI allowed".
- Optional per-host user override (for example, run AI commands as a restricted `ai` user, not `root`).
- Audit log of every call: time, host, command, mode, approver and exit code. It is stored encrypted in the vault.
- Kill switch: one click or `cli ai stop` ends all AI sessions.
- Output limits and redaction: cap output size, and mask strings that look like keys or tokens before the AI sees them.
- Prompt-injection warning: output from a server can contain text written to trick the AI. Approval mode is the main defence; document this risk for dangerous-mode users.

**Possible later setting:** auto-approve read-only commands (`ls`, `cat`, `df`, `systemctl status`) while still asking for everything else.

## Self-hosting for individuals and companies

One Docker Compose file should run the whole server for a single person. The same image should scale to a company deployment by changing configuration, not code.

| Need | Personal (v0.4) | Company (later) |
| --- | --- | --- |
| Deploy | `docker compose up`: server + PostgreSQL | Same image; external PostgreSQL; Kubernetes/Helm optional |
| Sign-in | Email + master password | Add OIDC/SAML SSO for sign-in; the master password still unlocks the vault |
| Users | One account | Invite-only signup, admin panel, disable or remove users |
| Sharing | None | Team vaults: item keys wrapped to each member's public key |
| Recovery | Personal recovery key | Optional org recovery key, opt-in by policy and clearly disclosed |
| Audit | Local AI audit log | Server-side event log (logins, devices, shares); contents stay encrypted |
| Config | `.env` file | Same env vars, plus secrets from a KMS or Vault |
| Backups | `pg_dump` script | Documented backup and restore; backups are already ciphertext |

**Design now so companies work later**

- Put an `org_id` on every table from day one, even if there is only one org.
- Keep the server stateless (sessions in PostgreSQL or Redis) so it can run as several replicas.
- Version the sync API (`/v1/...`) and the encrypted blob format (a version byte in every blob).
- Make every limit and feature flag an environment variable, such as signups on or off and the max devices per user.
- License: MIT for every component, so companies can adopt it with little legal review.

## Phases and tasks

The work splits into six phases. Each phase ends with something you can use yourself. Sizes assume part-time hobby work and include time to learn Rust, so treat them as rough guides, not deadlines.

### Phase 0: Foundations (about 2 weeks)

- [ ] Project name (OurShell) and license (MIT): decided
- [ ] Create the monorepo, Cargo workspace and GitHub Actions (build, test, `clippy`, `cargo audit`)
- [ ] Write a one-page threat model in `/docs`
- [ ] Write the encrypted blob format spec (version byte, nonce, ciphertext, wrapped key, signature)

### Phase 1: Core and local vault (about 6–8 weeks)

- [ ] Crypto module: Argon2id, XChaCha20-Poly1305, X25519 sealing, Ed25519 signing, with tests
- [ ] Vault: SQLite schema, create/unlock with master password, CRUD for hosts, identities, keys and snippets; local-only mode stays available after sync ships
- [ ] SSH: connect with password or key, known-hosts check, interactive PTY session
- [ ] SSH extras: jump host (ProxyJump), local and remote port forwarding, agent forwarding
- [ ] Key tools: generate ed25519/RSA, import an existing key with passphrase
- [ ] Importer for `~/.ssh/config`

### Phase 2: CLI, release v0.1 (about 3–4 weeks)

- [ ] Commands: `vault init/unlock/lock`, `host add/ls/rm/edit`, `connect <host>`, `key gen/import`, `forward`
- [ ] Agent daemon (the cli agent subcommand) with a versioned JSON-RPC API over a socket or named pipe: commands, PTY streams, events and an auto-lock timer
- [ ] SFTP basics: `get`, `put`, `ls`
- [ ] Install via `cargo install` and prebuilt binaries

### Phase 3: MCP server, release v0.2 (about 3–4 weeks)

- [ ] `mcp` crate on `rmcp`, stdio transport, tools `list_hosts` and `run_command`
- [ ] Policy engine: AI-allowed hosts, approval mode, dangerous mode scoped per session and host
- [ ] Approval prompt in the CLI (desktop prompt comes in Phase 4)
- [ ] Audit log, kill switch, output size limit and secret redaction
- [ ] SFTP tools `list_dir`, `read_file`, `write_file`
- [ ] Setup guide for Claude Desktop and Claude Code

### Phase 4: Desktop app, release v0.3 (about 8–10 weeks)

- [ ] Tauri 2 shell that bundles the agent binary as a sidecar and talks to it over JSON-RPC
- [ ] Vault unlock screen, host list, groups, search
- [ ] Terminal with `xterm.js`: tabs, split panes, themes, copy/paste
- [ ] SFTP file browser with drag and drop
- [ ] Port-forward manager and snippets panel
- [ ] AI panel: approval dialogs, dangerous-mode toggle with banner, audit log viewer
- [ ] Installers for Windows, macOS and Linux

### Phase 5: Sync server, release v0.4 (about 6–8 weeks)

- [ ] axum server crate in the Cargo workspace, sqlx migrations with `org_id` everywhere
- [ ] Signup and login with OPAQUE (opaque-ke on client and server), sessions, 2FA (TOTP); optional Google/OIDC sign-in that still requires the master password to unlock the vault
- [ ] Device registration and revocation with device public keys
- [ ] Sync API: push/pull encrypted items by version, conflict handling (last-write-wins with a conflict copy)
- [ ] Server-side encryption at rest, rate limits, request logging without secrets
- [ ] Docker Compose file and self-hosting guide
- [ ] Sync client in the Rust core, plus a recovery-key flow

### Phase 6: Hardening (ongoing)

- [ ] Fuzz the blob parser and protocol messages
- [ ] Signed builds and auto-update
- [ ] Ask the Rust and security communities for an outside review before calling it 1.0
- [ ] Termius migration: ssh config import plus a CSV importer modeled on Termius's CSV template
- [ ] Team vaults and company features

## Risks, open questions and next steps

The biggest risks are crypto mistakes and an AI running a harmful command, so both get specs and tests before features.

| Risk | Impact | Mitigation |
| --- | --- | --- |
| A flaw in the encryption design | All synced secrets exposed | Written spec first, standard libraries only, test vectors, outside review before 1.0 |
| AI runs a destructive command in dangerous mode | Data loss on a server | Opt-in per session and host, restricted AI user, audit log, kill switch |
| Prompt injection through command output | AI tricked into harmful actions | Approval mode by default; clear warning for dangerous mode |
| Learning Rust slows progress | Motivation drops | Start with the CLI; keep each release small and usable |
| Scope creep (teams, mobile, SSO) | Nothing ships | Stick to the release order; park ideas in a backlog |
| Lost master password | User loses the vault | Recovery key at signup, explained clearly |

**Open questions**

- [ ] Should dangerous mode also have a default time limit inside a session?
- [ ] Web UI inside Tauri: Svelte or React?
- [ ] Domain name and GitHub organization for OurShell?
- [ ] Should the agent run only while an app is open, or as a background service started at login?
- [ ] Termius migration: is ssh config import plus a CSV importer enough, since no export from the current Termius app was confirmed?

**Next steps**

1. Choose the name and license, and create the repo.
2. Write the threat model and blob format spec (Phase 0).
3. Build the crypto module with tests. It is the base for everything else.
