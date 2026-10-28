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
