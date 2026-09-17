#!/usr/bin/env bash
set -euo pipefail

# Restore a small, explicit subset from a verified archive ledger entry. Raw
# content remains local until compaction is separately enabled and verified.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if [[ -f "$ROOT/.env" ]]; then set -a; . "$ROOT/.env"; set +a; fi
DATABASE_URL="${DATABASE_URL:?DATABASE_URL is required}"
. "$ROOT/scripts/lib/pg-env.sh"
export PGPASSWORD="$(pg_password "$DATABASE_URL")"
DATABASE_URL="$(pg_url_strip_password "$DATABASE_URL")"
ARCHIVE_ID="${1:?Usage: $0 ARCHIVE_ID [LIMIT]}"
LIMIT="${2:-1}"
[[ "$ARCHIVE_ID" =~ ^[0-9a-fA-F-]{36}$ ]] || { echo "Invalid archive UUID" >&2; exit 2; }
[[ "$LIMIT" =~ ^[1-9][0-9]*$ ]] || { echo "LIMIT must be positive" >&2; exit 2; }

# Restore drill (decision #19, docs/WINDOWS_PORT_SYNTHESIS.md): verify the
# bundle Drive actually has for this archive still matches what was verified
# at archive time, and — for a .age bundle — that the configured identity can
# actually decrypt it, before trusting this archive enough to flip any row
# back to active. Local `documents.content` was never deleted by archiving
# today, so this is a proof step, not (yet) how content itself comes back.
# `|| true`: under set -e, a bare `read` returns nonzero (killing the script
# right here, before the very next line's own guard runs) whenever the query
# yields no rows — an unverified/typo'd/purged archive_id, not just a script bug.
IFS=$'\t' read -r bundle_path manifest_checksum < <(
  psql "$DATABASE_URL" -tA -F $'\t' -v ON_ERROR_STOP=1 -v archive_id="$ARCHIVE_ID" \
    -c "SELECT local_path, manifest_checksum FROM archive_meta.bundles WHERE archive_id = :'archive_id'::uuid AND state = 'verified'"
) || true
[[ -n "${bundle_path:-}" ]] || { echo "No verified bundle found for $ARCHIVE_ID" >&2; exit 1; }
BUNDLE_NAME="documents.ndjson"
[[ -f "$bundle_path/documents.ndjson.age" ]] && BUNDLE_NAME="documents.ndjson.age"
BUNDLE_FILE="$bundle_path/$BUNDLE_NAME"
[[ -f "$BUNDLE_FILE" ]] || { echo "Archive bundle missing on disk: $BUNDLE_FILE" >&2; exit 1; }
ACTUAL_SHA="$(shasum -a 256 "$BUNDLE_FILE" | awk '{print $1}')"
[[ "$ACTUAL_SHA" == "$manifest_checksum" ]] || { echo "Archive checksum mismatch for $ARCHIVE_ID — refusing to restore" >&2; exit 1; }
if [[ "$BUNDLE_NAME" == "documents.ndjson.age" ]]; then
  CRYPT_BIN="$ROOT/target/release/archive-crypt"
  [[ -x "$CRYPT_BIN" ]] || { echo "$BUNDLE_NAME is encrypted but $CRYPT_BIN is missing; build it first" >&2; exit 1; }
  SCRATCH="$(mktemp -d)"
  trap 'rm -rf "$SCRATCH"' EXIT
  "$CRYPT_BIN" decrypt "$BUNDLE_FILE" "$SCRATCH/documents.ndjson" || { echo "Failed to decrypt $BUNDLE_FILE — check ARCHIVE_AGE_IDENTITY_FILE" >&2; exit 1; }
  [[ -s "$SCRATCH/documents.ndjson" ]] || { echo "Decrypted bundle for $ARCHIVE_ID is empty" >&2; exit 1; }
fi

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
