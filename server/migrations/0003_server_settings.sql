-- Server settings (sign-up policy, address, admin password hash, OIDC, TLS,
-- proxy and backup settings) live in the database, so a deployment without
-- a persistent data volume (Dokploy, Kubernetes, ...) keeps them across
-- redeploys. Everything except the bootstrap keys (`database_url`,
-- `server_master_key`, `bind_addr`), serialized as JSON and encrypted with
-- the server master key (XChaCha20-Poly1305, AAD
-- `ourshell:server-settings:v1`, see `ourshell_server::settings::db`), so a
-- stolen dump doesn't leak the admin hash or the OIDC/S3/backup secrets.
-- The master key itself is never stored in the database.
CREATE TABLE server_settings (
    id SMALLINT PRIMARY KEY DEFAULT 1,
    encrypted_settings BYTEA NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT server_settings_singleton CHECK (id = 1)
);
