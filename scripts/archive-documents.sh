#!/usr/bin/env bash
set -euo pipefail

gen_uuid() {
  if command -v uuidgen >/dev/null 2>&1; then
    uuidgen | tr '[:upper:]' '[:lower:]'
  elif command -v powershell.exe >/dev/null 2>&1; then
    powershell.exe -NoProfile -Command "[guid]::NewGuid().ToString()" | tr -d '\r' | tr '[:upper:]' '[:lower:]'
  elif command -v python3 >/dev/null 2>&1; then
    python3 -c 'import uuid; print(uuid.uuid4())'
  else
    echo "No uuidgen, powershell.exe, or python3 available to generate a UUID" >&2
    exit 1
  fi
}

# Builds a portable, checksum-verified document bundle. It is dry-run by
# default; `--mark-archived` is deliberately separate from bundle creation.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if [[ -f "$ROOT/.env" ]]; then set -a; . "$ROOT/.env"; set +a; fi
DATABASE_URL="${DATABASE_URL:?DATABASE_URL is required}"
. "$ROOT/scripts/lib/pg-env.sh"
export PGPASSWORD="$(pg_password "$DATABASE_URL")"
DATABASE_URL="$(pg_url_strip_password "$DATABASE_URL")"
DRIVE_ROOT="${MEMORY_ARCHIVE_ROOT:-$HOME/Library/CloudStorage/GoogleDrive-humanoracle26@gmail.com/My Drive/memory-platform-archive}"
DEVICE_ID="${MEMORY_DEVICE_ID:-$(hostname | cut -d. -f1)}"
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

# Encryption is opt-in (decision #18/#19, docs/WINDOWS_PORT_SYNTHESIS.md):
# ARCHIVE_ENCRYPT=1 sends only the .age-encrypted bundle to Drive, never the
# plaintext ndjson. Default off so existing plaintext deployments are unchanged.
BUNDLE_NAME="documents.ndjson"
BUNDLE_FILE="$DATA"
if [[ "${ARCHIVE_ENCRYPT:-0}" == "1" ]]; then
  CRYPT_BIN="$ROOT/target/release/archive-crypt"
  [[ -x "$CRYPT_BIN" ]] || { echo "ARCHIVE_ENCRYPT=1 but $CRYPT_BIN is missing; build it first" >&2; exit 1; }
  "$CRYPT_BIN" encrypt "$DATA" "$DATA.age"
  BUNDLE_NAME="documents.ndjson.age"
  BUNDLE_FILE="$DATA.age"
fi

SHA="$(shasum -a 256 "$BUNDLE_FILE" | awk '{print $1}')"
ARCHIVE_ID=$(gen_uuid)
MANIFEST="$WORK/manifest.json"
printf '{"archive_id":"%s","device_id":"%s","section":"%s","records":%s,"sha256":"%s","created_at":"%s","bundle":"%s"}\n' "$ARCHIVE_ID" "$DEVICE_ID" "$SECTION" "$COUNT" "$SHA" "$STAMP" "$BUNDLE_NAME" > "$MANIFEST"
TARGET="$DRIVE_ROOT/$ARCHIVE_ID"
mkdir "$TARGET"
cp "$BUNDLE_FILE" "$MANIFEST" "$TARGET/"
[[ "$(shasum -a 256 "$TARGET/$BUNDLE_NAME" | awk '{print $1}')" == "$SHA" ]] || { echo "Archive checksum verification failed" >&2; exit 1; }

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
