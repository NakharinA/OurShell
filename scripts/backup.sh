#!/usr/bin/env bash
# Backs up the OurShell Postgres database with `pg_dump`.
#
# Every row this produces is already ciphertext, a public key, or sync
# metadata (see docs/threat-model.md) -- the server never stores anything
# that decrypts a user's vault -- but treat the backup as sensitive anyway:
# it is still enough to impersonate OPAQUE records and sessions if stolen,
# and it contains OPAQUE_SERVER_SETUP's encrypted copy.
#
# Usage:
#   ./scripts/backup.sh [output-dir]
#
# Reads the same .env the docker-compose.yml stack uses (or inherits
# already-exported DATABASE_URL / POSTGRES_* from the environment).
#
# Restore with:
#   gunzip -c ourshell-YYYYmmdd-HHMMSS.sql.gz | docker compose exec -T postgres \
#     psql -U "$POSTGRES_USER" -d "$POSTGRES_DB"

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

if [ -f .env ]; then
  set -a
  # shellcheck disable=SC1091
  source .env
  set +a
fi

OUT_DIR="${1:-./backups}"
mkdir -p "$OUT_DIR"

STAMP="$(date +%Y%m%d-%H%M%S)"
OUT_FILE="$OUT_DIR/ourshell-$STAMP.sql.gz"

POSTGRES_USER="${POSTGRES_USER:-ourshell}"
POSTGRES_DB="${POSTGRES_DB:-ourshell}"

echo "Backing up database '$POSTGRES_DB' to $OUT_FILE ..."

if docker compose ps postgres >/dev/null 2>&1 && [ -n "$(docker compose ps -q postgres 2>/dev/null)" ]; then
  # Preferred path: dump from inside the compose-managed container, so we
  # never need network access to Postgres from the host.
  docker compose exec -T postgres \
    pg_dump -U "$POSTGRES_USER" -d "$POSTGRES_DB" --no-owner --no-privileges \
    | gzip > "$OUT_FILE"
else
  # Fallback: dump over DATABASE_URL (e.g. an external/managed Postgres).
  : "${DATABASE_URL:?set DATABASE_URL or run this next to a running 'docker compose' postgres service}"
  pg_dump "$DATABASE_URL" --no-owner --no-privileges | gzip > "$OUT_FILE"
fi

echo "Done: $OUT_FILE ($(du -h "$OUT_FILE" | cut -f1))"

# Keep the last 14 backups by default; set OURSHELL_BACKUP_KEEP=0 to disable.
KEEP="${OURSHELL_BACKUP_KEEP:-14}"
if [ "$KEEP" -gt 0 ]; then
  ls -1t "$OUT_DIR"/ourshell-*.sql.gz 2>/dev/null | tail -n "+$((KEEP + 1))" | xargs -r rm -f
fi
