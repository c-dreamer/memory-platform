#!/usr/bin/env bash
set -euo pipefail

# sync-to-neon.sh - Sync local Postgres to Neon cloud.
# Default mode is incremental. Use --full for a destructive repair refresh.

LOCK_FILE="${LOCK_FILE:-/tmp/memory-platform-sync-to-neon.lock}"
STATE_FILE="${STATE_FILE:-/tmp/memory-platform-neon-sync-watermarks.tsv}"
OVERLAP_SECONDS="${OVERLAP_SECONDS:-5}"
LOCK_DIR="${LOCK_DIR:-${LOCK_FILE}.d}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR" && pwd)"

if [[ -f "$REPO_DIR/.env" ]]; then
  set -a
  # shellcheck disable=SC1091
  . "$REPO_DIR/.env"
  set +a
fi

LOCAL_URL="${LOCAL_URL:-${DATABASE_URL:-}}"
NEON_DIRECT="${NEON_DIRECT:-${NEON_DATABASE_URL:-}}"
SYNC_MODE="${SYNC_MODE:-incremental}"
TABLE_ORDER=(
  agents
  config
  projects
  sessions
  documents
  memories
  experiences
  procedures
  summaries
  code_changes
  trading_results
  contradictions
  relationships
  embeddings
  session_documents
  session_memories
)

while [[ $# -gt 0 ]]; do
  case "$1" in
    --full)
      SYNC_MODE="full"
      shift
      ;;
    --incremental)
      SYNC_MODE="incremental"
      shift
      ;;
    --state-file)
      STATE_FILE="${2:-}"
      shift 2
      ;;
    --overlap-seconds)
      OVERLAP_SECONDS="${2:-5}"
      shift 2
      ;;
    *)
      echo "[sync-to-neon] ERROR: Unknown argument: $1"
      exit 1
      ;;
  esac
done

if [[ -z "$LOCAL_URL" ]]; then
  echo "[sync-to-neon] ERROR: LOCAL_URL or DATABASE_URL must be set"
  exit 1
fi

if [[ -z "$NEON_DIRECT" ]]; then
  echo "[sync-to-neon] ERROR: NEON_DIRECT or NEON_DATABASE_URL must be set"
  exit 1
fi

mkdir -p "$(dirname "$STATE_FILE")"

acquire_lock() {
  if mkdir "$LOCK_DIR" 2>/dev/null; then
    printf '%s\n' "$$" >"$LOCK_DIR/pid"
    return 0
  fi

  if [[ -f "$LOCK_DIR/pid" ]]; then
    local existing_pid
    existing_pid="$(cat "$LOCK_DIR/pid" 2>/dev/null || true)"
    if [[ -n "$existing_pid" ]] && kill -0 "$existing_pid" 2>/dev/null; then
      echo "[sync-to-neon] Another sync is already running, exiting."
      exit 0
    fi

    rm -rf "$LOCK_DIR"
    if mkdir "$LOCK_DIR" 2>/dev/null; then
      printf '%s\n' "$$" >"$LOCK_DIR/pid"
      return 0
    fi
  fi

  echo "[sync-to-neon] Another sync is already running, exiting."
  exit 0
}

acquire_lock

cleanup() {
  [[ -n "${DUMP_FILE:-}" && -f "${DUMP_FILE:-}" ]] && rm -f "$DUMP_FILE"
  [[ -n "${TMP_SQL_FILE:-}" && -f "${TMP_SQL_FILE:-}" ]] && rm -f "$TMP_SQL_FILE"
  [[ -n "${BOOTSTRAP_FILE:-}" && -f "${BOOTSTRAP_FILE:-}" ]] && rm -f "$BOOTSTRAP_FILE"
  [[ -d "$LOCK_DIR" && "${LOCK_OWNER:-}" == "$$" ]] && rm -rf "$LOCK_DIR"
}
trap cleanup EXIT
LOCK_OWNER="$$"

sql_ident() {
  local value="$1"
  printf '"%s"' "${value//\"/\"\"}"
}

dump_url_for_container() {
  local value="$1"
  value="${value/127.0.0.1/host.docker.internal}"
  value="${value/localhost/host.docker.internal}"
  printf '%s' "$value"
}

get_saved_epoch() {
  local table="$1"
  if [[ ! -f "$STATE_FILE" ]]; then
    printf '0\n'
    return 0
  fi

  awk -F $'\t' -v table="$table" '
    $1 == table { value = $2 }
    END {
      if (value == "") {
        print 0
      } else {
        print value
      }
    }
  ' "$STATE_FILE"
}

save_state_value() {
  local table="$1"
  local epoch="$2"
  TMP_SQL_FILE="$(mktemp /tmp/memory-platform-neon-sync-state.XXXXXX.tsv)"
  if [[ -f "$STATE_FILE" ]]; then
    awk -F $'\t' -v OFS=$'\t' -v table="$table" -v epoch="$epoch" '
      $1 != table { print $1, $2 }
      END { print table, epoch }
    ' "$STATE_FILE" | sort >"$TMP_SQL_FILE"
  else
    printf '%s\t%s\n' "$table" "$epoch" >"$TMP_SQL_FILE"
  fi
  mv "$TMP_SQL_FILE" "$STATE_FILE"
  TMP_SQL_FILE=""
}

schema_bootstrap_needed() {
  local table
  for table in "${TABLE_ORDER[@]}"; do
    if [[ "$(psql "$NEON_DIRECT" -t -A -c \
      "SELECT EXISTS (
         SELECT 1
         FROM information_schema.tables
         WHERE table_schema = 'public'
           AND table_type = 'BASE TABLE'
           AND table_name = '$table'
       );")" != "t" ]]; then
      return 0
    fi
  done
  return 1
}

bootstrap_schema() {
  echo "[sync-to-neon] Bootstrapping Neon core schema..."
  psql "$NEON_DIRECT" -v ON_ERROR_STOP=1 -f migrations/001_initial.sql >/dev/null
  psql "$NEON_DIRECT" -v ON_ERROR_STOP=1 -f migrations/002_hybrid_decay_contradiction.sql >/dev/null
  psql "$NEON_DIRECT" -v ON_ERROR_STOP=1 -f migrations/003_session_vault_xref.sql >/dev/null
}

list_public_tables() {
  printf '%s\n' "${TABLE_ORDER[@]}"
}

get_table_columns() {
  local table="$1"
  psql "$LOCAL_URL" -t -A -c \
    "SELECT column_name
     FROM information_schema.columns
     WHERE table_schema = 'public'
       AND table_name = '$table'
     ORDER BY ordinal_position;"
}

get_primary_key_column() {
  local table="$1"
  psql "$LOCAL_URL" -t -A -c \
    "SELECT a.attname
     FROM pg_index i
     JOIN pg_class c ON c.oid = i.indrelid
     JOIN pg_namespace n ON n.oid = c.relnamespace
     JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum = ANY(i.indkey)
     WHERE n.nspname = 'public'
       AND c.relname = '$table'
       AND i.indisprimary
     ORDER BY array_position(i.indkey, a.attnum);"
}

get_watermark_column() {
  local table="$1"
  psql "$LOCAL_URL" -t -A -c \
    "SELECT column_name
     FROM information_schema.columns
     WHERE table_schema = 'public'
       AND table_name = '$table'
       AND column_name IN ('updated_at', 'accessed_at', 'created_at', 'last_seen_at', 'started_at')
     ORDER BY CASE column_name
       WHEN 'updated_at' THEN 1
       WHEN 'accessed_at' THEN 2
       WHEN 'created_at' THEN 3
       WHEN 'last_seen_at' THEN 4
       WHEN 'started_at' THEN 5
       ELSE 99
     END
     LIMIT 1;"
}

max_watermark_epoch() {
  local table="$1"
  local watermark_col="$2"
  local table_sql
  local col_sql
  table_sql=$(sql_ident "$table")
  col_sql=$(sql_ident "$watermark_col")
  psql "$LOCAL_URL" -t -A -c \
    "SELECT COALESCE(FLOOR(EXTRACT(EPOCH FROM MAX(${col_sql})))::BIGINT, 0)
     FROM public.${table_sql};"
}

row_count_since() {
  local table="$1"
  local watermark_col="$2"
  local epoch="$3"
  local table_sql
  local col_sql
  table_sql=$(sql_ident "$table")
  col_sql=$(sql_ident "$watermark_col")
  psql "$LOCAL_URL" -t -A -c \
    "SELECT COUNT(*)
     FROM public.${table_sql}
     WHERE ${col_sql} >= to_timestamp($epoch);"
}

build_on_conflict_clause() {
  local table="$1"
  local pk_col="$2"
  local columns=()
  local update_parts=()
  local col

  while IFS= read -r col; do
    [[ -z "$col" ]] && continue
    columns+=("$col")
  done < <(get_table_columns "$table")

  for col in "${columns[@]}"; do
    [[ "$col" == "$pk_col" ]] && continue
    update_parts+=("$(sql_ident "$col") = EXCLUDED.$(sql_ident "$col")")
  done

  if [[ ${#update_parts[@]} -eq 0 ]]; then
    printf 'ON CONFLICT (%s) DO NOTHING' "$(sql_ident "$pk_col")"
  else
    printf 'ON CONFLICT (%s) DO UPDATE SET %s' \
      "$(sql_ident "$pk_col")" \
      "$(IFS=', '; echo "${update_parts[*]}")"
  fi
}

dump_table_upserts() {
  local table="$1"
  local watermark_col="$2"
  local where_epoch="$3"
  local pk_col="$4"
  local on_conflict="$5"
  local local_dump_url
  local table_sql
  local watermark_sql
  local pk_sql
  local stage_sql
  local data_file
  local query_sql

  local_dump_url="$(dump_url_for_container "$LOCAL_URL")"
  table_sql=$(sql_ident "$table")
  watermark_sql=$(sql_ident "$watermark_col")
  pk_sql=$(sql_ident "$pk_col")
  stage_sql=$(sql_ident "__sync_stage_${table}")
  data_file="$(mktemp /tmp/memory-platform-neon-sync-table.XXXXXX.csv)"
  query_sql="COPY (SELECT * FROM public.${table_sql} WHERE ${watermark_sql} >= to_timestamp(${where_epoch}) ORDER BY ${watermark_sql}, ${pk_sql}) TO STDOUT WITH (FORMAT csv, NULL '\\N')"
  DUMP_FILE="$data_file"

  psql "$NEON_DIRECT" -v ON_ERROR_STOP=1 <<SQL >/dev/null
DROP TABLE IF EXISTS public.${stage_sql};
CREATE UNLOGGED TABLE public.${stage_sql} (LIKE public.${table_sql} INCLUDING DEFAULTS INCLUDING GENERATED INCLUDING IDENTITY);
SQL

  psql "$LOCAL_URL" -Atc "$query_sql" >"$data_file"
  psql "$NEON_DIRECT" -v ON_ERROR_STOP=1 -c "\\copy ${stage_sql} FROM '${data_file}' WITH (FORMAT csv, NULL '\\N')"

  psql "$NEON_DIRECT" -v ON_ERROR_STOP=1 <<SQL >/dev/null
INSERT INTO public.${table_sql}
SELECT * FROM ${stage_sql}
${on_conflict};
SQL

  psql "$NEON_DIRECT" -v ON_ERROR_STOP=1 <<SQL >/dev/null
DROP TABLE IF EXISTS public.${stage_sql};
SQL
}

run_full_sync() {
  local local_dump_url
  local_dump_url="$(dump_url_for_container "$LOCAL_URL")"

  DUMP_FILE="$(mktemp /tmp/memory-platform-neon-sync.XXXXXX.sql)"
  echo "[sync-to-neon] Creating full dump at $DUMP_FILE"
  docker run --rm --add-host=host.docker.internal:host-gateway postgres:18 \
    pg_dump "$local_dump_url" --schema=public --no-owner --no-acl \
    >"$DUMP_FILE"

  echo "[sync-to-neon] Resetting Neon schema..."
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
  sed '/^CREATE SCHEMA public;$/d' "$DUMP_FILE" \
    | psql "$NEON_DIRECT" -v ON_ERROR_STOP=1 >/dev/null

  echo "[sync-to-neon] Verification:"
  while IFS= read -r table; do
    [[ -z "$table" ]] && continue
    count=$(psql "$NEON_DIRECT" -t -A -c "SELECT count(*) FROM public.$(sql_ident "$table");")
    echo "[sync-to-neon]   ${table}: ${count}"
  done < <(list_public_tables)

  NEON_SIZE=$(psql "$NEON_DIRECT" -t -A -c "SELECT pg_size_pretty(pg_database_size(current_database()));")
  echo "[sync-to-neon] Neon DB size: $NEON_SIZE"
  echo "[sync-to-neon] $(date): Full sync complete."
}

run_incremental_sync() {
  echo "[sync-to-neon] Checking Neon schema..."
  if schema_bootstrap_needed; then
    bootstrap_schema
  fi

  echo "[sync-to-neon] Incremental mode enabled."
  echo "[sync-to-neon] Using overlap window of ${OVERLAP_SECONDS}s for safety."

  while IFS= read -r table; do
    [[ -z "$table" ]] && continue

    local_pk=""
    local_watermark=""
    local_current_epoch=0
    local_last_epoch=0
    local_query_epoch=0

    local_pk=$(get_primary_key_column "$table")
    local_watermark=$(get_watermark_column "$table")

    if [[ -z "$local_pk" || -z "$local_watermark" ]]; then
      echo "[sync-to-neon] Skipping ${table}: missing primary key or watermark column"
      continue
    fi

    local_current_epoch=$(max_watermark_epoch "$table" "$local_watermark")
    local_last_epoch=$(get_saved_epoch "$table")

    if (( local_current_epoch <= local_last_epoch )); then
      echo "[sync-to-neon] ${table}: no changes"
      continue
    fi

    local_query_epoch=$(( local_last_epoch > OVERLAP_SECONDS ? local_last_epoch - OVERLAP_SECONDS : 0 ))
    changed_count=$(row_count_since "$table" "$local_watermark" "$local_query_epoch")

    echo "[sync-to-neon] ${table}: changed rows since watermark = ${changed_count}"
    if [[ "$changed_count" == "0" ]]; then
      save_state_value "$table" "$local_current_epoch"
      continue
    fi

    on_conflict_clause=$(build_on_conflict_clause "$table" "$local_pk")
    dump_table_upserts "$table" "$local_watermark" "$local_query_epoch" "$local_pk" "$on_conflict_clause"

    save_state_value "$table" "$local_current_epoch"
  done < <(list_public_tables)

  NEON_SIZE=$(psql "$NEON_DIRECT" -t -A -c "SELECT pg_size_pretty(pg_database_size(current_database()));")
  echo "[sync-to-neon] Neon DB size: $NEON_SIZE"
  echo "[sync-to-neon] $(date): Incremental sync complete."
}

echo "[sync-to-neon] $(date): Starting sync..."

if ! psql "$LOCAL_URL" -c "SELECT 1" >/dev/null 2>&1; then
  echo "[sync-to-neon] ERROR: Cannot connect to local Postgres"
  exit 1
fi

if ! psql "$NEON_DIRECT" -c "SELECT 1" >/dev/null 2>&1; then
  echo "[sync-to-neon] ERROR: Cannot connect to Neon"
  exit 1
fi

LOCAL_SIZE=$(psql "$LOCAL_URL" -t -A -c "SELECT pg_size_pretty(pg_database_size(current_database()));")
echo "[sync-to-neon] Local DB size: $LOCAL_SIZE"

case "$SYNC_MODE" in
  full)
    run_full_sync
    ;;
  incremental)
    run_incremental_sync
    ;;
  *)
    echo "[sync-to-neon] ERROR: Unknown SYNC_MODE: $SYNC_MODE"
    exit 1
    ;;
esac
