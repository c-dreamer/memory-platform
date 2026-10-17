#!/usr/bin/env bash
# Rebuild an EMPTY local store from a Neon dump, e.g. on a freshly set-up
# device. The local database is this device's offline write cache and holds
# rows Neon does not (unsynced outbox entries, archived records), so this
# refuses to touch a local database that has data unless
# --confirm-overwrite-local is given. Never run implicitly.
set -euo pipefail

usage() { echo "Usage: $0 [--confirm-overwrite-local]" >&2; exit 2; }
CONFIRM_OVERWRITE=0
for arg in "$@"; do
  case "$arg" in
    --confirm-overwrite-local) CONFIRM_OVERWRITE=1 ;;
    *) usage ;;
  esac
done

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_DIR"

if [[ -f "$REPO_DIR/.env" ]]; then
  set -a
  # shellcheck disable=SC1091
  . "$REPO_DIR/.env"
  set +a
fi

LOCAL_URL="${LOCAL_URL:-${DATABASE_URL:-}}"
NEON_URL="${NEON_URL:-${NEON_DATABASE_URL:-}}"

if [[ -z "$LOCAL_URL" ]]; then
  echo "[rehydrate] ERROR: LOCAL_URL or DATABASE_URL must be set"
  exit 1
fi

if [[ -z "$NEON_URL" ]]; then
  echo "[rehydrate] ERROR: NEON_URL or NEON_DATABASE_URL must be set"
  exit 1
fi

# The restore does not carry the local capture triggers (they never exist on
# Neon), so `neon-sync migrate` must reinstall them before anything writes
# locally. Check its inputs now, before the local store is replaced.
if [[ -z "${MEMORY_DEVICE_ID:-}" ]] ||
   [[ -z "${SYNC_TARGET_URL:-${NEON_DIRECT:-${NEON_SYNC_URL:-}}}" ]]; then
  echo "[rehydrate] ERROR: MEMORY_DEVICE_ID and SYNC_TARGET_URL (or NEON_DIRECT/NEON_SYNC_URL) must be set;"
  echo "[rehydrate]        neon-sync migrate needs them to reinstall local capture triggers after the restore"
  exit 1
fi

# cargo run (below) needs the full URLs with password embedded (sqlx has no
# PGPASSWORD fallback), so LOCAL_URL/NEON_URL stay intact for that; only the
# pg_dump/psql argv below gets password-free URLs plus PGPASSWORD.
. "$SCRIPT_DIR/lib/pg-env.sh"
NEON_PW="$(pg_password "$NEON_URL")"
LOCAL_PW="$(pg_password "$LOCAL_URL")"
NEON_URL_NOPASS="$(pg_url_strip_password "$NEON_URL")"
LOCAL_URL_NOPASS="$(pg_url_strip_password "$LOCAL_URL")"

local_psql() { PGPASSWORD="$LOCAL_PW" psql "$LOCAL_URL_NOPASS" -X -v ON_ERROR_STOP=1 "$@"; }

# Rows in any public table (except the migration ledger), queued outbox
# entries, and the cold-archive ledger. Anything non-zero is local data the
# restore would destroy.
LOCAL_ROWS="$(local_psql -Atq -c "
  SELECT coalesce(sum((xpath('/row/c/text()', query_to_xml(
           format('SELECT count(*) AS c FROM %I.%I', schemaname, tablename), false, true, '')))[1]::text::bigint), 0)
  FROM pg_tables
  WHERE (schemaname = 'public' AND tablename <> '_migrations')
     OR (schemaname = 'sync_meta' AND tablename = 'outbox')
     OR schemaname = 'archive_meta'")"

if [[ "$LOCAL_ROWS" != "0" ]]; then
  if [[ "$CONFIRM_OVERWRITE" != 1 ]]; then
    echo "[rehydrate] REFUSING: local database $LOCAL_URL_NOPASS holds $LOCAL_ROWS row(s), including any unsynced"
    echo "[rehydrate]           sync_meta.outbox entries and archived records that Neon does not have."
    echo "[rehydrate]           Push them first (./sync-to-neon.sh run), or rerun with --confirm-overwrite-local"
    echo "[rehydrate]           to discard all local data and replace it with the Neon projection."
    exit 3
  fi
  echo "[rehydrate] --confirm-overwrite-local: discarding $LOCAL_ROWS local row(s)"
fi

DUMP_FILE="$(umask 077 && mktemp "${TMPDIR:-/tmp}/rehydrate.XXXXXX")"
trap 'rm -f "$DUMP_FILE"' EXIT

# Dump to a file first: a failed or partial dump never reaches the local DB.
# Host pg_dump when installed (its major version must be >= Neon's), else the
# pgvector/pgvector:pg17 container.
echo "[rehydrate] dumping Neon: $NEON_URL_NOPASS"
if command -v pg_dump >/dev/null 2>&1; then
  PGPASSWORD="$NEON_PW" pg_dump "$NEON_URL_NOPASS" --no-owner --no-acl >"$DUMP_FILE"
else
  PGPASSWORD="$NEON_PW" docker run --rm -e PGPASSWORD \
    pgvector/pgvector:pg17 \
    pg_dump "$NEON_URL_NOPASS" --no-owner --no-acl >"$DUMP_FILE"
fi

# One transaction: wipe the schemas migrations create (as a fresh database
# would be) and load the dump. Any error rolls everything back, leaving the
# local DB exactly as it was. pg_dump --clean is not used: it only drops
# objects Neon has, so it fails on local-only sync_meta objects and leaves
# stale ones.
echo "[rehydrate] restoring local store from Neon into: $LOCAL_URL_NOPASS"
{
  echo "SET client_min_messages = warning;"
  echo "DROP SCHEMA IF EXISTS sync_meta CASCADE;"
  echo "DROP SCHEMA IF EXISTS archive_meta CASCADE;"
  echo "DROP SCHEMA IF EXISTS public CASCADE;"
  echo "CREATE SCHEMA public;"
  echo "GRANT USAGE ON SCHEMA public TO PUBLIC;"
  sed '/pg_session_jwt/d' "$DUMP_FILE"
} | local_psql --single-transaction -q >/dev/null

echo "[rehydrate] reinstalling local capture triggers (neon-sync migrate)"
DATABASE_URL="$LOCAL_URL" cargo run --quiet --bin neon-sync -- migrate
# The restored sync_meta.events includes this device's own events; advance the
# Lamport clock past them (the same rule neon-sync pull applies) so new local
# captures do not collide on UNIQUE(device_id, logical_time).
local_psql -q -c "SELECT setval('sync_meta.logical_clock', GREATEST((SELECT last_value FROM sync_meta.logical_clock), (SELECT max(logical_time) FROM sync_meta.events)))" >/dev/null

echo "[rehydrate] ingesting current local source files"
DATABASE_URL="$LOCAL_URL" cargo run --quiet --bin ingest -- all

echo "[rehydrate] local vs neon summary"
LOCAL_URL="$LOCAL_URL" NEON_URL="$NEON_URL" cargo run --quiet --bin stats -- --compare LOCAL_URL NEON_URL
