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
| `OURSHELL_TLS_DOMAIN` | no | unset | serve HTTPS with a Let's Encrypt certificate for this domain (see below) |
| `OURSHELL_TLS_EMAIL` | no | unset | contact email for the Let's Encrypt account |
| `OURSHELL_TLS_BIND` | no | `0.0.0.0:8444` | where the built-in HTTPS listener binds |
| `OURSHELL_ACME_STAGING` | no | `false` | `1`/`true`/`yes`/`on` = Let's Encrypt staging (untrusted test certificates) |

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

## HTTPS with Let's Encrypt

The server can get and renew its own certificate from Let's Encrypt. Set a
domain on the admin page (or `OURSHELL_TLS_DOMAIN`, plus optionally
`OURSHELL_TLS_EMAIL` for expiry notices) and, once the server is set up, it
serves the same API and pages over HTTPS on `OURSHELL_TLS_BIND` (default
`0.0.0.0:8444`) in addition to plain HTTP on `BIND_ADDR`. Plain HTTP keeps
running so LAN clients, the setup wizard and the container healthcheck
work unchanged; point clients at `https://<your domain>` for anything that
crosses the internet. Changing or clearing the domain on the admin page
takes effect immediately, without a restart.

Requirements:

- **DNS**: an A (and/or AAAA) record for the domain pointing at the host's
  public address.
- **Port 443**: Let's Encrypt validates with the TLS-ALPN-01 challenge,
  which connects to port 443 on that address. The container runs as a
  non-root user, so it listens on 8444 instead: forward host port 443 to
  container port 8444 (e.g. `ports: ["443:8444"]` in `docker-compose.yml`,
  plus a router port forward if the host is behind NAT). Port 80 is not
  needed.
- **Persistent data dir**: the ACME account key and certificates are cached
  in `<data dir>/acme` (`$OURSHELL_DATA_DIR`, `/data` in the Docker image).
  Keep it on a volume; otherwise every restart orders a new certificate and
  you will soon hit Let's Encrypt's rate limits.
- **Testing**: set `OURSHELL_ACME_STAGING=1` while you get DNS and port
  forwarding right. Staging certificates aren't trusted by browsers or the
  app, but the rate limits are far more generous. Unset it afterwards (the
  production certificate is cached separately).

The admin page's status shows `tls_status` ("waiting for certificate",
"active", or the last error); the server log has the details (`ACME:`
lines). Renewal happens automatically in the background.

Prefer your own reverse proxy (Caddy, nginx, Traefik, a cloud load
balancer)? Leave the domain empty and terminate TLS there, forwarding to
`BIND_ADDR` over plain HTTP and setting `X-Forwarded-Proto: https`, which
the admin page uses to mark its session cookie `Secure`.

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

`ourshell vault passwd` changes the master password (updating the server
too if signed in -- see "Changing the master password" below).

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

### Changing the master password

`ourshell vault passwd` changes the master password. If the vault isn't
already unlocked it asks for the current one first (to unlock locally --
this never leaves the device); the new password is then set locally
(`Vault::change_password`, a fresh KDF salt) and, if the vault is signed
in, pushed to the server: a brand-new OPAQUE registration replaces the
account's password file, and the account's current KDF params and
wrapped account keys replace the server's copy of them, atomically. Every
*other* session for that account is revoked in the same step -- the usual
"signed out everywhere else" behavior of a password change -- so other
devices need to `ourshell account login` again with the new password
(logging in refreshes their copy of the account keys from the server).
The device making the change keeps its own session.

If the network step fails (no connectivity, server down), the master
password has still changed *locally* -- `ourshell vault passwd` reports
this and re-running it is safe (it just registers under the same new
password again).

### Recovering a vault that is signed in to a server

`ourshell vault recover` resets the master password from the 24-word
recovery phrase (`Vault::recover`). A vault created from this point on
also wraps its account signing/encryption keypair under the recovery key
(`header.recovery_account_wrap`), so recovering restores the *same*
account keys instead of generating new ones -- no re-signing, and no
desync with a sync server or another device, which is the gap this
closes relative to earlier versions.

If the vault is signed in, `vault recover` also pushes the (unchanged)
account keys' current KDF params and wrapped copy to the server in the
same step `vault passwd` uses, and revokes every other session, exactly
like a password change -- which is accurate: forgetting your password and
recovering it is a password reset. This needs a *valid session*, so if
the session on this device had already expired or been revoked, recovery
still succeeds locally but the CLI prints a warning instead of silently
leaving the server on the old password: log in with the OLD password
first if you still remember it (then run `vault recover` again), or ask
an org admin to disable (and re-invite) the account if you don't.

**Vaults created before this field existed** have no recovery copy of
their account keys and fall back to the original behaviour: `recover`
generates a brand-new account keypair and re-signs every item under it.
If such a vault is signed in to a server, this is a narrower version of
the previous known gap: the server (and every other device) still has
the *old* verifying key on file until you also push the regenerated one
(the CLI still attempts the same server push, which updates
`enc_public`/`sign_public` too in this case -- but other already-synced
devices will fail to verify this device's future signatures, and this
device will fail to verify theirs, until they also `account login` again
to pick up the new keys). The practical workaround, as before, is to sign
out everywhere and sign back in.

## Scaling to a team

- **Invite-only**: set `INVITE_ONLY=true`. An owner/admin creates codes with
  `ourshell org invite` (`POST /v1/orgs/invites`, optionally
  `{"role": "admin"}`); each code is single-use.
- **Admin panel**: `ourshell org users` (`GET /v1/orgs/users`) lists
  everyone in the org; `ourshell org disable <user-id>`
  (`POST /v1/orgs/users/{id}/disable`) disables an account and kills its
  sessions.
- **Team vaults**: `ourshell team create <name> [--member email]...`
  generates a vault key client-side and seals a copy to yourself and each
  named member's public key (looked up with
  `GET /v1/users/{email}/public-keys`), then `POST /v1/vaults` (the server
  only ever stores the sealed bytes). `ourshell team ls` lists every vault
  (personal and team) this account can open; `ourshell host ls` shows
  which vault each host lives in. Existing members add others with
  `ourshell team add-member <team> <email>`
  (`POST /v1/vaults/{vault_id}/members`) and remove them with
  `ourshell team remove-member <team> <email>`
  (`DELETE /vaults/{vault_id}/members/{user_id}`) -- removing a member
  also *rotates* the vault's key (a fresh key, re-sealed to every
  remaining member, every item re-saved under it), so the removed member
  -- and any device that hasn't synced since -- can no longer decrypt
  anything pushed from that point on. `ourshell team share <host> <team>`
  moves a personal item into a team vault (under a new id -- see
  `docs/blob-format.md`).
- **Audit log**: every login, device add/revoke, password change, and
  vault create/share/unshare is recorded in the `events` table (time,
  org, user, kind) -- never secret contents.

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
| `GET /account/vaults` | session | refreshes the caller's vault list (picks up a new team membership, or a removal, without a fresh login) |
| `POST /account/password/start` / `/finish` | session | `ourshell vault passwd` / `vault recover` while signed in; fresh OPAQUE registration for the current user, replaces `kdf`/`wrapped_account_keys`/public keys/recovery wrap, revokes every *other* session |
| `GET /vaults/{vault_id}/items?since=<cursor>` | session, vault member | pull |
| `POST /vaults/{vault_id}/items` | session, vault member | push; optimistic concurrency (`ok` or `conflict` per item) |
| `POST /orgs/invites` | session, admin/owner | |
| `GET /orgs/users` | session, admin/owner | |
| `POST /orgs/users/{user_id}/disable` | session, admin/owner | |
| `POST /vaults` | session | create a team vault |
| `GET /vaults/{vault_id}/members` | session, vault member | every current member's public keys (verify signatures, reseal on rotation) |
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
(`sqlx`'s Postgres driver, the optional OIDC HTTP client, and the built-in
Let's Encrypt HTTPS listener via `rustls-acme`) is configured
to use the pure-Rust `ring` crypto provider instead. `cargo tree -i
aws-lc-sys` should always report no matches for this workspace.
