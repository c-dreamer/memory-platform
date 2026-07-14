#!/usr/bin/env bash
set -euo pipefail

# Restore a small, explicit subset from a verified archive ledger entry. Raw
# content remains local until compaction is separately enabled and verified.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if [[ -f "$ROOT/.env" ]]; then set -a; . "$ROOT/.env"; set +a; fi
DATABASE_URL="${DATABASE_URL:?DATABASE_URL is required}"
ARCHIVE_ID="${1:?Usage: $0 ARCHIVE_ID [LIMIT]}"
LIMIT="${2:-1}"
[[ "$ARCHIVE_ID" =~ ^[0-9a-fA-F-]{36}$ ]] || { echo "Invalid archive UUID" >&2; exit 2; }
[[ "$LIMIT" =~ ^[1-9][0-9]*$ ]] || { echo "LIMIT must be positive" >&2; exit 2; }

psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -v archive_id="$ARCHIVE_ID" -v limit="$LIMIT" <<'SQL'
WITH selected AS (
  SELECT r.record_key
  FROM archive_meta.records r
  JOIN archive_meta.bundles b ON b.archive_id = r.archive_id
  WHERE r.archive_id = :'archive_id'::uuid
    AND b.state = 'verified'
    AND r.table_name = 'documents'
    AND r.state = 'archived'
  ORDER BY r.record_key
  LIMIT :'limit'
), restored AS (
  UPDATE documents d SET storage_tier = 'active'
  FROM selected s
  WHERE d.id::text = s.record_key AND d.storage_tier = 'archived'
  RETURNING d.id::text
)
UPDATE archive_meta.records r SET state = 'restored', restored_at = now()
FROM restored x
WHERE r.archive_id = :'archive_id'::uuid AND r.table_name = 'documents' AND r.record_key = x.id;
SQL
echo "Restored up to $LIMIT documents locally. The outbox will mirror active records to Neon."
