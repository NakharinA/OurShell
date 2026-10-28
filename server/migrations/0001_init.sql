-- OurShell sync server schema (v1).
--
-- Every per-tenant table carries `org_id`, even though phase 5 only ever
-- creates one org (the first registered user becomes its owner). This keeps
-- the door open for real multi-org hosting later without a migration.
--
-- The server never stores plaintext secrets: `items.envelope` and
-- `totp.secret` are encrypted at rest with the server master key
-- (XChaCha20-Poly1305, see `ourshell_server::crypto`); everything else
-- client-sealed (`opaque_records.password_file`, `account_keys.wrapped_account_keys`,
-- `vault_members.sealed_vault_key`, ...) is already ciphertext the server
-- cannot open.

CREATE TABLE orgs (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- `email` is always stored trimmed and lower-cased; it is also the OPAQUE
-- credential identifier, so client and server must agree on the same
-- normalization.
CREATE TABLE users (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    email TEXT NOT NULL,
    role TEXT NOT NULL DEFAULT 'member', -- owner | admin | member
    disabled BOOLEAN NOT NULL DEFAULT false,
    totp_enabled BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, email)
);

CREATE TABLE opaque_records (
    user_id UUID PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    org_id UUID NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    password_file BYTEA NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE account_keys (
    user_id UUID PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    org_id UUID NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    enc_public TEXT NOT NULL,
    sign_public TEXT NOT NULL,
    kdf TEXT NOT NULL,
    wrapped_account_keys BYTEA NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE devices (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    public_key TEXT NOT NULL,
    revoked BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE sessions (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    device_id UUID NOT NULL REFERENCES devices (id) ON DELETE CASCADE,
    token_hash BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    UNIQUE (token_hash)
);

CREATE TABLE vaults (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    kind TEXT NOT NULL, -- personal | team
    name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE vault_members (
    vault_id UUID NOT NULL REFERENCES vaults (id) ON DELETE CASCADE,
    org_id UUID NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    key_id TEXT NOT NULL,
    sealed_vault_key BYTEA NOT NULL,
    recovery_wrap BYTEA,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (vault_id, user_id)
);

CREATE TABLE items (
    vault_id UUID NOT NULL REFERENCES vaults (id) ON DELETE CASCADE,
    org_id UUID NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    id UUID NOT NULL,
    kind TEXT NOT NULL,
    version BIGINT NOT NULL,
    deleted BOOLEAN NOT NULL DEFAULT false,
    envelope BYTEA NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    seq BIGSERIAL,
    PRIMARY KEY (vault_id, id)
);

CREATE INDEX items_vault_seq_idx ON items (vault_id, seq);

CREATE TABLE totp (
    user_id UUID PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    org_id UUID NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    secret BYTEA NOT NULL,
    confirmed BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE invites (
    code TEXT PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    role TEXT NOT NULL DEFAULT 'member',
    created_by UUID REFERENCES users (id) ON DELETE SET NULL,
    used_by UUID REFERENCES users (id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    used_at TIMESTAMPTZ
);

CREATE TABLE events (
    id BIGSERIAL PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    user_id UUID REFERENCES users (id) ON DELETE SET NULL,
    kind TEXT NOT NULL,
    detail TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Implementation detail tables: short-lived protocol state that must live in
-- Postgres (not process memory) so the server stays stateless across
-- replicas. Rows are deleted as soon as they're consumed; a background sweep
-- isn't implemented for v1 since callers always consume-and-delete, but the
-- `created_at` column is kept for an eventual cron cleanup of abandoned ones.

CREATE TABLE pending_registrations (
    id UUID PRIMARY KEY,
    email TEXT NOT NULL,
    invite_code TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE pending_logins (
    id UUID PRIMARY KEY,
    user_id UUID REFERENCES users (id) ON DELETE CASCADE,
    email TEXT NOT NULL,
    server_login_state BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE server_setup (
    id SMALLINT PRIMARY KEY DEFAULT 1,
    encrypted_setup BYTEA NOT NULL,
    CONSTRAINT server_setup_singleton CHECK (id = 1)
);

CREATE TABLE oidc_requests (
    state TEXT PRIMARY KEY,
    nonce TEXT NOT NULL,
    pkce_verifier TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
