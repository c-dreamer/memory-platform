#!/usr/bin/env bash
set -euo pipefail

# Verification is intentionally local/read-only: archive creation and compaction
# always require an explicit operator command after a reviewable dry run.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if [[ -f "$ROOT/.env" ]]; then set -a; . "$ROOT/.env"; set +a; fi
DATABASE_URL="${DATABASE_URL:?DATABASE_URL is required}"
DRIVE_ROOT="${MEMORY_ARCHIVE_ROOT:-$HOME/Library/CloudStorage/GoogleDrive-humanoracle26@gmail.com/My Drive/memory-platform-archive}"
[[ -d "$DRIVE_ROOT" ]] || { echo "Archive mount unavailable: $DRIVE_ROOT" >&2; exit 1; }

psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -P pager=off <<'SQL'
SELECT state, count(*) AS bundles FROM archive_meta.bundles GROUP BY state ORDER BY state;
SELECT storage_tier, count(*) AS documents FROM documents GROUP BY storage_tier ORDER BY storage_tier;
SELECT count(*) AS pending_neon_operations FROM sync_meta.outbox;
SQL

failed=0
while IFS=$'\t' read -r archive_id manifest_checksum; do
  path="$DRIVE_ROOT/$archive_id"
  if [[ ! -f "$path/documents.ndjson" || ! -f "$path/manifest.json" ]]; then
    echo "Missing archive files for $archive_id" >&2; failed=1; continue
  fi
  actual="$(shasum -a 256 "$path/documents.ndjson" | awk '{print $1}')"
  if [[ "$actual" != "$manifest_checksum" ]]; then
    echo "Checksum mismatch for $archive_id" >&2; failed=1
  fi
done < <(psql "$DATABASE_URL" -tA -F $'\t' -v ON_ERROR_STOP=1 -c "SELECT archive_id, manifest_checksum FROM archive_meta.bundles WHERE state='verified'")

(( failed == 0 )) || exit 1
echo "Archive verification passed."
