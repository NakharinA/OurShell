# OurShell threat model

One page. Revisit before every release.

## Assets

| Asset | Where it lives |
| --- | --- |
| SSH private keys, passwords, passphrases | Vault items (encrypted) |
| Host inventory (hostnames, IPs, users, notes) | Vault items (encrypted) |
| Master password | The user's head only |
| Unlocked keys (master, account, vault, item) | Agent process memory only, zeroized on lock |
| Device private key | OS keychain |
| AI audit log | Vault (encrypted) |

## Adversaries and what they get

| Adversary | Capability | What we guarantee |
| --- | --- | --- |
| Thief of the server DB or backups | Reads every row | Only ciphertext, public keys, OPAQUE records and sync metadata. No secrets, and no offline password guessing without the OPAQUE server key. |
| Malicious or compromised server admin | Reads and changes the DB, serves modified responses | Cannot decrypt. Tampering is caught by signatures over every item. Can deny service; per-item version checks catch rollback. |
| Network attacker against the sync server | MITM between client and server | TLS required. OPAQUE gives mutual auth, so no password material leaks even if TLS fails. |
| Network attacker against SSH | MITM between client and host | Known-hosts check. Unknown keys need explicit trust; changed keys are refused. |
| Malicious AI, or prompt injection via command output | Calls MCP tools | Only "AI allowed" hosts; human approval by default; dangerous mode scoped to one session, chosen hosts and a time limit; audit log; kill switch; output redaction. |
| Another local user | Reads files on disk | Vault file is encrypted; agent socket restricted to the owning user. |
| Local malware running as the user | Reads memory, logs keys | **Out of scope.** No client-side design defends against this. |

## Out of scope

- Compromised OS or hardware, malicious extensions in the desktop webview, side channels.
- Recovering a forgotten master password without a recovery key. The data is lost by design.

## Accepted dependency risks

| Advisory | Why it is accepted |
| --- | --- |
| RUSTSEC-2023-0071 (`rsa` Marvin timing attack) | RSA is used only on the client to generate keys and sign SSH auth challenges. There is no RSA decryption and no remote timing oracle. Ed25519 is the default. Re-check when a fixed `rsa` release ships. |
| RUSTSEC-2026-0387 (`crypto-bigint` 0.5 timing) | Only reachable through `elliptic-curve` 0.13, a mandatory dependency of `opaque-ke` 4.0 (our suite is ristretto255 on `curve25519-dalek`), and through `openidconnect`, which verifies public ID-token signatures. No secret-dependent P-curve arithmetic runs. Upgrade when both move to `elliptic-curve` 0.14 or later. |

## Design rules

1. The server never holds a key that decrypts user data.
2. Only well-known crypto crates; no custom primitives.
3. Unique random 24-byte nonces (XChaCha20-Poly1305).
4. Key material lives in `zeroize`/`secrecy` types.
5. Every blob carries a version byte so formats can change safely.
