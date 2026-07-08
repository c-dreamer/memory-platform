#!/usr/bin/env bash
set -euo pipefail

# sync-to-neon.sh - Sync local Postgres to Neon cloud.
# Intended to run under cron or a systemd timer.

LOCK_FILE="${LOCK_FILE:-/tmp/memory-platform-sync-to-neon.lock}"

if [[ -f "$(dirname "$0")/.env" ]]; then
  set -a
  # shellcheck disable=SC1091
  . "$(dirname "$0")/.env"
  set +a
fi

LOCAL_URL="${LOCAL_URL:-${DATABASE_URL:-postgres://memory:YAft44tZyrG4DET0WeigY8BpZ%252BcqGgPtTXsPK4XFgXc%253D@127.0.0.1:5433/memory}}"
NEON_DIRECT="${NEON_DIRECT:-${NEON_DATABASE_URL:-}}"

if [[ -z "$NEON_DIRECT" ]]; then
  echo "[sync-to-neon] ERROR: NEON_DIRECT or NEON_DATABASE_URL must be set"
  exit 1
fi
TABLES=(
  documents
  memories
  experiences
  sessions
  agents
  projects
  relationships
  procedures
  summaries
  code_changes
  trading_results
  contradictions
  config
  embeddings
)

exec 9>"$LOCK_FILE"
if ! flock -n 9 2>/dev/null; then
  echo "[sync-to-neon] Another sync is already running, exiting."
  exit 0
fi

cleanup() {
  [[ -n "${DUMP_FILE:-}" && -f "${DUMP_FILE:-}" ]] && rm -f "$DUMP_FILE"
  [[ -n "${FILTERED_DUMP_FILE:-}" && -f "${FILTERED_DUMP_FILE:-}" ]] && rm -f "$FILTERED_DUMP_FILE"
}
trap cleanup EXIT

echo "[sync-to-neon] $(date): Starting sync..."

if ! psql "$LOCAL_URL" -c "SELECT 1" >/dev/null 2>&1; then
  echo "[sync-to-neon] ERROR: Cannot connect to local Postgres"
  exit 1
fi

if ! psql "$NEON_DIRECT" -c "SELECT 1" >/dev/null 2>&1; then
  echo "[sync-to-neon] ERROR: Cannot connect to Neon"
  exit 1
fi

LOCAL_SIZE=$(psql "$LOCAL_URL" -t -A -c "SELECT pg_size_pretty(pg_database_size('memory'));")
echo "[sync-to-neon] Local DB size: $LOCAL_SIZE"

DUMP_FILE="$(mktemp /tmp/memory-platform-neon-sync.XXXXXX.sql)"
FILTERED_DUMP_FILE="$(mktemp /tmp/memory-platform-neon-sync-filtered.XXXXXX.sql)"
echo "[sync-to-neon] Creating dump at $DUMP_FILE"
pg_dump "$LOCAL_URL" --schema=public --no-owner --no-acl >"$DUMP_FILE"
sed '/^CREATE SCHEMA public;$/d' "$DUMP_FILE" >"$FILTERED_DUMP_FILE"

echo "[sync-to-neon] Resetting Neon schema..."
for table in "${TABLES[@]}"; do
  psql "$NEON_DIRECT" -v ON_ERROR_STOP=1 -c "DROP TABLE IF EXISTS public.${table} CASCADE;" >/dev/null
done
psql "$NEON_DIRECT" -v ON_ERROR_STOP=1 <<'SQL' >/dev/null
DO $$
DECLARE
  relation record;
  drop_kind text;
BEGIN
  FOR relation IN
    SELECT c.oid::regclass AS signature, c.relkind
    FROM pg_class c
    JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname = 'public'
      AND c.relkind IN ('r', 'p', 'v', 'm', 'S', 'f')
      AND NOT EXISTS (
        SELECT 1
        FROM pg_depend d
        WHERE d.objid = c.oid
          AND d.classid = 'pg_class'::regclass
          AND d.deptype = 'e'
      )
  LOOP
    drop_kind := CASE relation.relkind
      WHEN 'S' THEN 'SEQUENCE'
      WHEN 'v' THEN 'VIEW'
      WHEN 'm' THEN 'MATERIALIZED VIEW'
      WHEN 'f' THEN 'FOREIGN TABLE'
      ELSE 'TABLE'
    END;
    EXECUTE format('DROP %s IF EXISTS %s CASCADE', drop_kind, relation.signature);
  END LOOP;
END $$;
SQL
psql "$NEON_DIRECT" -v ON_ERROR_STOP=1 <<'SQL' >/dev/null
DO $$
DECLARE
  routine record;
BEGIN
  FOR routine IN
    SELECT p.oid::regprocedure AS signature
    FROM pg_proc p
    JOIN pg_namespace n ON n.oid = p.pronamespace
    WHERE n.nspname = 'public'
      AND NOT EXISTS (
        SELECT 1
        FROM pg_depend d
        WHERE d.objid = p.oid
          AND d.classid = 'pg_proc'::regclass
          AND d.deptype = 'e'
      )
  LOOP
    EXECUTE format('DROP ROUTINE IF EXISTS %s CASCADE', routine.signature);
  END LOOP;
END $$;
SQL

echo "[sync-to-neon] Restoring dump to Neon..."
psql "$NEON_DIRECT" -v ON_ERROR_STOP=1 -f "$FILTERED_DUMP_FILE" >/dev/null

echo "[sync-to-neon] Verification:"
for table in "${TABLES[@]}"; do
  count=$(psql "$NEON_DIRECT" -t -A -c "SELECT count(*) FROM public.${table};")
  echo "[sync-to-neon]   ${table}: ${count}"
done

NEON_SIZE=$(psql "$NEON_DIRECT" -t -A -c "SELECT pg_size_pretty(pg_database_size('neondb'));")
echo "[sync-to-neon] Neon DB size: $NEON_SIZE"
echo "[sync-to-neon] $(date): Sync complete."
