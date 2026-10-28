-- Support for changing the master password (and recovering while signed
-- in) without desyncing other devices: the account's enc/sign secret keys
-- can now also be wrapped under the vault's recovery key, so a recovery
-- restores the *same* keys instead of regenerating them. The server
-- stores that wrap too (opaque to it, like `wrapped_account_keys`) so a
-- brand-new device logging in also gets the ability to recover.
ALTER TABLE account_keys ADD COLUMN recovery_account_wrap BYTEA;

-- Short-lived protocol state for `POST /v1/account/password/start` +
-- `/finish`: a fresh OPAQUE registration for the *current* (authenticated)
-- user, consumed and deleted by `finish`, same lifecycle as
-- `pending_registrations`.
CREATE TABLE pending_password_changes (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
