#!/usr/bin/env bash
# Backs up an OurShell sync server: a `pg_dump` of its Postgres database,
# plus the server's config.toml (which holds SERVER_MASTER_KEY -- a database
# backup can't be restored without it).
#
# Every row this produces is already ciphertext, a public key, or sync
# metadata (see docs/threat-model.md) -- the server never stores anything
# that decrypts a user's vault -- but treat the backup as sensitive anyway:
# it is still enough to impersonate OPAQUE records and sessions if stolen,
# and config.toml holds the server's master key (which also decrypts the
# server settings stored in the database, like the admin password hash).
#
# Usage:
#   ./backup.sh [output-dir]            # in an install dir made by install-server.sh
#   ./scripts/backup.sh [output-dir]    # in a repo checkout (docker-compose.yml at the root)
#
# It works in $OURSHELL_DIR, else the directory this script is in, when that
# directory has a docker-compose.yml and .env (an installer-made install
# dir); otherwise in the repository root, as before. It reads that .env (or
# inherits already-exported DATABASE_URL / POSTGRES_* from the environment).
#
# Output: <output-dir>/ourshell-<stamp>.sql.gz and config-<stamp>.toml
# (default output dir: ./backups inside that directory).
#
# Restore (bundled `postgres` service), into an empty database:
#   gunzip -c ourshell-YYYYmmdd-HHMMSS.sql.gz | docker compose exec -T postgres \
#     psql -U "${POSTGRES_USER:-ourshell}" -d "${POSTGRES_DB:-ourshell}"
#   docker exec -i ourshell-server sh -c 'umask 077; cat > /data/config.toml' \
#     < config-YYYYmmdd-HHMMSS.toml
#   docker compose restart server
# (`docker exec` writes it as the server's non-root user; `docker cp` would
# leave it owned by root, unreadable to the server at mode 600.)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CANDIDATE="${OURSHELL_DIR:-$SCRIPT_DIR}"
if [ -f "$CANDIDATE/docker-compose.yml" ] && [ -f "$CANDIDATE/.env" ]; then
  cd "$CANDIDATE"
else
  cd "$SCRIPT_DIR/.."
fi

if [ -f .env ]; then
  set -a
  # shellcheck disable=SC1091
  source .env
  set +a
fi

OUT_DIR="${1:-./backups}"
mkdir -p "$OUT_DIR"
chmod 700 "$OUT_DIR" 2>/dev/null || true
umask 077

STAMP="$(date +%Y%m%d-%H%M%S)"
OUT_FILE="$OUT_DIR/ourshell-$STAMP.sql.gz"

POSTGRES_USER="${POSTGRES_USER:-ourshell}"
POSTGRES_DB="${POSTGRES_DB:-ourshell}"

if [ -n "$(docker compose ps -q postgres 2>/dev/null)" ]; then
  # Preferred path: dump from inside the compose-managed container, so we
  # never need network access to Postgres from the host.
  echo "Backing up database '$POSTGRES_DB' to $OUT_FILE ..."
  docker compose exec -T postgres \
    pg_dump -U "$POSTGRES_USER" -d "$POSTGRES_DB" --no-owner --no-privileges \
    | gzip > "$OUT_FILE"
else
  # An existing/external Postgres: dump over DATABASE_URL.
  : "${DATABASE_URL:?set DATABASE_URL or run this next to a running 'docker compose' postgres service}"
  echo "Backing up $DATABASE_URL (password hidden)" | sed 's#://\([^:/@]*\):[^@]*@#://\1:***@#'
  echo "  to $OUT_FILE ..."
  case "$DATABASE_URL" in
    *host.docker.internal*) use_container=1 ;;
    *) if command -v pg_dump >/dev/null 2>&1; then use_container=0; else use_container=1; fi ;;
  esac
  if [ "$use_container" = 1 ]; then
    # No usable pg_dump on the host (or the URL only resolves inside
    # Docker): run one in a throwaway container.
    docker run --rm --add-host host.docker.internal:host-gateway postgres:17 \
      pg_dump "$DATABASE_URL" --no-owner --no-privileges \
      | gzip > "$OUT_FILE"
  else
    pg_dump "$DATABASE_URL" --no-owner --no-privileges | gzip > "$OUT_FILE"
  fi
fi
echo "Done: $OUT_FILE ($(du -h "$OUT_FILE" | cut -f1))"

# config.toml: written by the setup wizard into the server's /data volume.
CONFIG_FILE="$OUT_DIR/config-$STAMP.toml"
SERVER_ID="$(docker compose ps -q server 2>/dev/null || true)"
SERVER_ID="${SERVER_ID:-ourshell-server}"
if docker cp "$SERVER_ID:/data/config.toml" "$CONFIG_FILE" >/dev/null 2>&1; then
  chmod 600 "$CONFIG_FILE" 2>/dev/null || true
  echo "Done: $CONFIG_FILE (contains the master key -- keep it as safe as the dump)"
else
  echo "warning: couldn't copy /data/config.toml out of the server container;" >&2
  echo "         if this server is configured only through .env, back up SERVER_MASTER_KEY from there." >&2
fi

# Keep the last 14 backups by default; set OURSHELL_BACKUP_KEEP=0 to disable.
KEEP="${OURSHELL_BACKUP_KEEP:-14}"
if [ "$KEEP" -gt 0 ]; then
  for pattern in 'ourshell-*.sql.gz' 'config-*.toml'; do
    # shellcheck disable=SC2012,SC2086 # $pattern is a glob on purpose; names are our own timestamped files
    ls -1t "$OUT_DIR"/$pattern 2>/dev/null | tail -n "+$((KEEP + 1))" | while IFS= read -r old; do
      rm -f "$old"
    done
  done
fi
