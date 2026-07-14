#!/usr/bin/env bash
set -euo pipefail

# Builds a portable, checksum-verified document bundle. It is dry-run by
# default; `--mark-archived` is deliberately separate from bundle creation.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if [[ -f "$ROOT/.env" ]]; then set -a; . "$ROOT/.env"; set +a; fi
DATABASE_URL="${DATABASE_URL:?DATABASE_URL is required}"
DRIVE_ROOT="${MEMORY_ARCHIVE_ROOT:-$HOME/Library/CloudStorage/GoogleDrive-humanoracle26@gmail.com/My Drive/memory-platform-archive}"
DEVICE_ID="${MEMORY_DEVICE_ID:-$(hostname -s)}"
SECTION=".playwright-mcp"
MARK_ARCHIVED=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --section) SECTION="$2"; shift 2 ;;
    --mark-archived) MARK_ARCHIVED=true; shift ;;
    --help) echo "Usage: $0 [--section NAME] [--mark-archived]"; exit 0 ;;
    *) echo "Unknown option: $1" >&2; exit 2 ;;
  esac
done

[[ -d "$DRIVE_ROOT" || -d "$(dirname "$DRIVE_ROOT")" ]] || { echo "Google Drive mount is unavailable: $DRIVE_ROOT" >&2; exit 1; }
[[ "$SECTION" =~ ^[A-Za-z0-9._@[:space:]-]+$ ]] || { echo "Unsafe section name" >&2; exit 2; }
SECTION_SQL="${SECTION//\'/\'\'}"
COUNT="$(psql "$DATABASE_URL" -tA -v ON_ERROR_STOP=1 -c "SELECT count(*) FROM documents WHERE vault_section = '$SECTION_SQL' AND storage_tier = 'active';")"
echo "Archive candidates: $COUNT documents in section '$SECTION'"
[[ "$COUNT" != "0" ]] || exit 0
[[ "$MARK_ARCHIVED" == true ]] || { echo "Dry run only. Re-run with --mark-archived after reviewing this candidate set."; exit 0; }

STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
WORK="${XDG_STATE_HOME:-$HOME/.local/state}/memory-platform/archive/$STAMP"
mkdir -p "$WORK" "$DRIVE_ROOT"
DATA="$WORK/documents.ndjson"
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -c "\\copy (SELECT jsonb_build_object('table','documents','id',id,'path',path,'checksum',checksum,'content',content,'frontmatter',frontmatter,'created_at',created_at,'updated_at',updated_at) FROM documents WHERE vault_section = '$SECTION_SQL' AND storage_tier='active' ORDER BY id) TO '$DATA'"
SHA="$(shasum -a 256 "$DATA" | awk '{print $1}')"
ARCHIVE_ID="$(uuidgen | tr '[:upper:]' '[:lower:]')"
MANIFEST="$WORK/manifest.json"
printf '{"archive_id":"%s","device_id":"%s","section":"%s","records":%s,"sha256":"%s","created_at":"%s"}\n' "$ARCHIVE_ID" "$DEVICE_ID" "$SECTION" "$COUNT" "$SHA" "$STAMP" > "$MANIFEST"
TARGET="$DRIVE_ROOT/$ARCHIVE_ID"
mkdir "$TARGET"
cp "$DATA" "$MANIFEST" "$TARGET/"
[[ "$(shasum -a 256 "$TARGET/documents.ndjson" | awk '{print $1}')" == "$SHA" ]] || { echo "Archive checksum verification failed" >&2; exit 1; }

psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -v archive_id="$ARCHIVE_ID" -v section="$SECTION" -v path="$TARGET" -v sha="$SHA" -v count="$COUNT" -v device="$DEVICE_ID" <<'SQL'
INSERT INTO archive_meta.bundles(archive_id,local_path,remote_path,manifest_checksum,byte_count,state,verified_at)
VALUES (:'archive_id'::uuid, :'path', :'path', :'sha', pg_size_bytes(:'count' || ' bytes'), 'verified', now());
INSERT INTO archive_meta.records(archive_id,table_name,record_key,source_checksum,reason,device_id,state)
SELECT :'archive_id'::uuid, 'documents', id::text, checksum, 'generated-section', :'device', 'archived'
FROM documents WHERE vault_section=:'section' AND storage_tier='active';
UPDATE documents SET storage_tier='archived', archive_id=:'archive_id'::uuid, source_checksum=checksum
WHERE vault_section=:'section' AND storage_tier='active';
SQL
echo "Verified archive $ARCHIVE_ID at $TARGET; records are now excluded from Neon and retained locally for restore."
