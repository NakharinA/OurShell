# Request for an outside security review

OurShell is an open-source, self-hostable SSH client with a zero-knowledge sync server and an MCP server that lets AI run commands on hosts the user approves. Before calling it 1.0, we ask the Rust and security communities to review it. This page says what to look at and how to report findings.

**Report vulnerabilities privately** through GitHub Security Advisories on the repository ("Report a vulnerability"). Please do not open public issues for security bugs.

## Priority areas

| # | Area | Code | Questions |
| --- | --- | --- | --- |
| 1 | Key hierarchy and envelopes | `crates/core/src/crypto/`, `docs/blob-format.md` | Are AAD and signature bindings sufficient against server-side swapping, replay and rollback of items? Any nonce reuse risk? Is the sealed-box construction (X25519 + HKDF-SHA256 + XChaCha20-Poly1305) sound as specified? |
| 2 | OPAQUE login | `crates/proto/src/opaque.rs`, `server/src/routes/` (auth), `server/src/auth.rs` | Cipher suite choice; server setup key handling; user enumeration through timing or error differences; session token storage (hash only). |
| 3 | Vault at rest | `crates/core/src/vault/` | Is anything secret stored in plain text in the SQLite file? Recovery flow (`Vault::recover`) re-keys the account keys and re-signs items; is it correct? |
| 4 | Sync server trust | `server/src/routes/`, `server/src/crypto.rs` | Can a malicious server do more than deny service or roll back (which we detect per item)? Is team-vault membership handled correctly when members are removed (key rotation)? |
| 5 | Local agent | `crates/cli/src/agent/`, `crates/cli/src/ipc.rs`, `docs/agent-protocol.md` | Socket/pipe access control on each OS; can another local user connect? Can an MCP client reach non-AI methods or widen its host set? |
| 6 | AI policy | `crates/cli/src/agent/policy.rs`, `crates/cli/src/agent/ai.rs` | Approval bypasses, dangerous-mode scoping (session, hosts, expiry), kill switch, audit completeness, redaction gaps. |
| 7 | SSH client | `crates/core/src/ssh/` | Host-key verification (TOFU, mismatch refusal, hashed known_hosts), jump-host chains, agent forwarding exposure. |
| 8 | Desktop app | `apps/desktop/src-tauri/`, `tauri.conf.json`, `capabilities/` | CSP, Tauri capability scope (fs scope), rendering of untrusted terminal and SFTP data, updater signature verification. |
| 9 | Supply chain | `Cargo.lock`, `apps/desktop/package-lock.json`, `.github/workflows/` | Dependency choices, CI permissions, release signing and provenance. |

## Known limitations (no need to report)

- Malware running as the same OS user can read agent memory while the vault is unlocked.
- Prompt injection through command output is mitigated by approval mode, not prevented. Dangerous mode is documented as risky.
- A forgotten master password with no recovery key means the data is lost, by design.
- The server can withhold updates (denial of service) and can see metadata: item ids, types, versions, timestamps and sizes.

## How to build and test

```sh
cargo test --workspace                      # server DB tests need Postgres; see docs/self-hosting.md
cargo +nightly fuzz run envelope_json       # from fuzz/ (also: aead_decrypt, seal_open, agent_rpc, sync_messages, ssh_config, termius_csv)
cd apps/desktop && npm ci && npm run check && npm test
```

Threat model: [`docs/threat-model.md`](threat-model.md).
