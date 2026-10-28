# Self-hosting the OurShell sync server

One Docker Compose stack runs the whole server for a single person; the
same image scales to a small company by changing environment variables,
not code. See the project plan's "Self-hosting for individuals and
companies" section for the design goals, and `docs/threat-model.md` for
what the server can and can't see.

## Quick start

```sh
cp .env.example .env
# Fill in SERVER_MASTER_KEY (openssl rand -base64 32) and POSTGRES_PASSWORD.
docker compose up -d
curl http://localhost:8443/v1/health
```

The first account anyone registers automatically becomes the **owner** of
the server's single org, regardless of `SIGNUPS_ENABLED`/`INVITE_ONLY`
(there's no one else yet to issue an invite). Every signup after that
follows the normal policy.

## What the server never sees

Hostnames, credentials, SSH keys, passphrases, snippets and notes are
encrypted on the client before they ever reach the server (see
`docs/blob-format.md`). The server additionally layers its own
`SERVER_MASTER_KEY` (XChaCha20-Poly1305) over two columns at rest --
`items.envelope` and `totp.secret` -- so a stolen disk or database dump
doesn't even hand over the client-side ciphertext's row metadata in the
clear. This is a second, independent layer for "someone stole my backups",
not the thing that keeps the server zero-knowledge: **losing
`SERVER_MASTER_KEY` does not expose vault contents**, and the server's key
can never decrypt what the client encrypted.

## Configuration

All configuration is environment variables; see `.env.example` for the
full, commented list and `server/src/config.rs` for how each is parsed.

| Variable | Required | Default | Notes |
| --- | --- | --- | --- |
| `DATABASE_URL` | yes | - | Postgres connection string |
| `BIND_ADDR` | no | `0.0.0.0:8443` | |
| `SERVER_MASTER_KEY` | yes | - | base64, 32 random bytes |
| `OPAQUE_SERVER_SETUP` | no | generated once, persisted encrypted in `server_setup` | only set by hand to restore one explicitly |
| `SIGNUPS_ENABLED` | no | `true` | |
| `INVITE_ONLY` | no | `false` | |
| `MAX_DEVICES_PER_USER` | no | `10` | `0` = unlimited |
| `RATE_LIMIT_PER_MIN` | no | `30` | per client IP, auth endpoints only; `0` = unlimited |
| `SESSION_TTL_HOURS` | no | `720` (30 days) | |
| `OIDC_ISSUER` / `OIDC_CLIENT_ID` / `OIDC_CLIENT_SECRET` / `OIDC_REDIRECT_URL` | no | unset | all four together enable `/v1/oidc/*` |

### Generating `SERVER_MASTER_KEY`

```sh
openssl rand -base64 32
```

### OIDC / SSO

Setting all four `OIDC_*` variables turns on `GET /v1/oidc/start` (redirects
to your provider) and `GET /v1/oidc/callback` (exchanges the code, verifies
the ID token, and reports whether that email already has an account). OIDC
only **identifies** the user; the master password still unlocks the vault
over OPAQUE afterward, same as a normal login. If unset, both routes answer
`501 Not Implemented`.

## Connecting clients

The `ourshell` CLI (and anything built on `ourshell_core::sync`) talks to
this server over the API below. The local agent holds the session; the CLI
is a thin wrapper around it (`ourshell agent` starts on demand, same as for
every other command).

```sh
# First device: create a local vault, add some hosts, then sign up --
# this uploads that vault's *existing* key material, nothing is regenerated.
ourshell vault init
ourshell host add prod-web root@203.0.113.10
ourshell account signup --server https://ourshell.example.com --email me@example.com
ourshell sync

# A second device: no local vault yet (or an empty one). Logging in builds
# one from the server's material and decrypts everything already pushed.
ourshell account login --server https://ourshell.example.com --email me@example.com
ourshell sync

# Afterwards, on either device:
ourshell account status
ourshell account devices
ourshell account revoke <device-id>
ourshell account logout
```

`ourshell account login` refuses to run over a local vault that already has
items (it would otherwise discard them) -- move that file aside first if
you really mean to replace it. `--server` also reads `$OURSHELL_SERVER`, and
master passwords honor `$OURSHELL_PASSWORD` like every other prompt (see
`docs/agent-protocol.md`).

Two-factor authentication: `ourshell account totp setup` prints an
`otpauth://` URL and a manual-entry secret; scan or type it into an
authenticator app, then `ourshell account totp confirm <code>` turns it on
(`totp disable <code>` turns it back off). Once enabled, `account login`
needs `--totp <code>` too.

**Auto-sync**: while a vault is unlocked and signed in, the agent
periodically runs the same push/pull `ourshell sync` does, every
`$OURSHELL_AUTOSYNC_SECS` seconds (default 300; `0` disables it).

**Conflicts**: pushing is optimistic-concurrency (`version = current + 1`
or the push is rejected). On a conflict the server's copy always wins for
that item id, but the local edit is never silently dropped -- it is kept as
a new item (renamed "<name> (conflict copy)" when the item has a `name`
field) so you can merge by hand. Deleting an item pushes a tombstone, which
every other device applies the same way.

**What's persisted where**: the session token, this device's id and its
Ed25519 identity, and the pull cursor live *inside* the vault's own SQLite
file, encrypted with the vault key (`Vault::set_secret_meta`/
`get_secret_meta`, AEAD-sealed, base64 in the `meta` table) -- never in
plain text, and never synced to the server. This is a deliberate deviation
from the project plan's suggestion of the OS keychain: it keeps the client
free of a keychain dependency while still encrypting the token at rest, at
the cost of tying that session to this one vault file (copying the vault
file to another machine also copies its session -- revoke the device if
that's not what you want).

### Recovering a vault that is signed in to a server

`ourshell vault recover` resets the master password from the 24-word
recovery phrase (`Vault::recover`). Locally this regenerates the vault's
account signing/encryption keypair and re-signs every item with it -- by
design, so a lost password never requires trusting the old one.

If that vault is signed in to a sync server, this is currently a known
gap: the server (and every other device) still has the **old** account
keys on file (`enc_public`/`sign_public`/`wrapped_account_keys`/`kdf` in
the `account_keys` table), and there is no `/v1/...` endpoint to update
them after the fact -- registration only writes that row once, at signup.
After a recovery, other devices' `apply_remote` checks will fail to verify
the signature on anything *this* device pushes afterward (and this device
will fail to verify items signed before the recovery), because the
verifying key changed out from under the account. The CLI prints a loud
warning when this happens. Until a `change_password`/`rotate_keys`
endpoint exists, the practical workaround is: sign out everywhere
(`ourshell account logout` on each device), then `ourshell account signup`
again as if setting up sync for the first time.

## Scaling to a team

- **Invite-only**: set `INVITE_ONLY=true`. An owner/admin creates codes with
  `POST /v1/orgs/invites` (optionally `{"role": "Admin"}`); each code is
  single-use.
- **Admin panel (API)**: `GET /v1/orgs/users` lists everyone in the org;
  `POST /v1/orgs/users/{id}/disable` disables an account and kills its
  sessions.
- **Team vaults**: any member can create one with `POST /v1/vaults`,
  sealing a copy of the vault key to each initial member's public key
  client-side (the server only ever stores the sealed bytes). Look up a
  teammate's public keys first with `GET /v1/users/{email}/public-keys`.
  Existing members add others with `POST /v1/vaults/{vault_id}/members` and
  remove them with `DELETE /v1/vaults/{vault_id}/members/{user_id}`.
- **Audit log**: every login, device add/revoke, and vault share/unshare is
  recorded in the `events` table (time, org, user, kind) -- never secret
  contents.

## API reference

All endpoints are under `/v1`, JSON in/out (see `crates/proto/src/sync.rs`
for exact request/response shapes). Authenticated endpoints take
`Authorization: Bearer <session_token>`.

| Method & path | Auth | Notes |
| --- | --- | --- |
| `GET /health` | none | |
| `POST /register/start` / `/register/finish` | none | OPAQUE registration |
| `POST /login/start` / `/login/finish` | none | OPAQUE login; `login/finish` takes an optional `totp` code |
| `POST /logout` | session | |
| `GET /devices` | session | this user's devices |
| `POST /devices/{device_id}/revoke` | session | also deletes that device's sessions |
| `POST /totp/setup` | session | |
| `POST /totp/confirm` | session | |
| `POST /totp/disable` | session | |
| `GET /vaults/{vault_id}/items?since=<cursor>` | session, vault member | pull |
| `POST /vaults/{vault_id}/items` | session, vault member | push; optimistic concurrency (`ok` or `conflict` per item) |
| `POST /orgs/invites` | session, admin/owner | |
| `GET /orgs/users` | session, admin/owner | |
| `POST /orgs/users/{user_id}/disable` | session, admin/owner | |
| `POST /vaults` | session | create a team vault |
| `POST /vaults/{vault_id}/members` | session, vault member | |
| `DELETE /vaults/{vault_id}/members/{user_id}` | session, vault member | |
| `GET /users/{email}/public-keys` | session, same org | |
| `GET /oidc/start` / `GET /oidc/callback` | none | 501 unless `OIDC_*` is configured |

## Backups

```sh
./scripts/backup.sh            # writes ./backups/ourshell-<timestamp>.sql.gz
```

Backups are a `pg_dump` of ciphertext, public keys and sync metadata (see
above); restoring is a normal `pg_restore`/`psql` import, documented in the
script's header. Keep backups anyway as if they were sensitive: they still
contain enough to impersonate OPAQUE records and sessions if someone steals
one.

## Running multiple replicas

The server keeps no state in process memory except the (configurable)
per-IP rate limiter -- sessions, OPAQUE protocol state, and the OPAQUE
server setup itself all live in Postgres -- so you can run several
replicas behind a load balancer with no sticky sessions required, as long
as they share one `DATABASE_URL` and `SERVER_MASTER_KEY`. Only the rate
limiter is per-replica (acceptable: worst case a client gets a slightly
higher effective rate limit than configured, never a security issue).

## Dependency note: why there's no NASM/aws-lc-sys

This server is built without `aws-lc-sys` on purpose -- it needs NASM on
Windows and is a heavier native build everywhere else. Every TLS consumer
(`sqlx`'s Postgres driver, and the optional OIDC HTTP client) is configured
to use the pure-Rust `ring` crypto provider instead. `cargo tree -i
aws-lc-sys` should always report no matches for this workspace.
