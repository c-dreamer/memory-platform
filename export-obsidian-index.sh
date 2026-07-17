#!/usr/bin/env bash
set -euo pipefail

# export-obsidian-index.sh — Export a curated project index from the Main Obsidian vault
# via its REST API. Output is redacted JSON — no raw note bodies.
#
# Required env vars:
#   OBSIDIAN_MAIN_API_URL       — base URL of the Obsidian REST API (e.g. http://localhost:27123)
#   OBSIDIAN_API_KEY            — API key for the Obsidian REST API
#   OBSIDIAN_PROJECT_INDEX_PATH — API path for the curated project index (e.g. /vault/project-index)
#
# Output: writes to OBSIDIAN_EXPORT_PATH (default: ~/Documents/AI/outputs/obsidian-project-index.json)

export PATH="$HOME/.local/bin:$HOME/bin:/usr/local/bin:/usr/bin:/bin"

ENV_FILE="${ENV_FILE:-$HOME/memory-platform-rust/.env}"
if [ -f "$ENV_FILE" ]; then
  set -a
  source "$ENV_FILE"
  set +a
fi

: "${OBSIDIAN_MAIN_API_URL:=}"
: "${OBSIDIAN_API_KEY:=}"
: "${OBSIDIAN_PROJECT_INDEX_PATH:=}"
: "${OBSIDIAN_EXPORT_PATH:=${AI_ROOT:-$HOME/Documents/AI}/outputs/obsidian-project-index.json}"

if [ -z "$OBSIDIAN_MAIN_API_URL" ] || [ -z "$OBSIDIAN_API_KEY" ] || [ -z "$OBSIDIAN_PROJECT_INDEX_PATH" ]; then
  echo '[export-obsidian-index] ERROR: OBSIDIAN_MAIN_API_URL, OBSIDIAN_API_KEY, and OBSIDIAN_PROJECT_INDEX_PATH are required'
  exit 1
fi

if [[ "$OBSIDIAN_PROJECT_INDEX_PATH" != /* ]]; then
  echo '[export-obsidian-index] ERROR: OBSIDIAN_PROJECT_INDEX_PATH must be an absolute API path'
  exit 1
fi

OUTPUT_DIR="$(dirname "$OBSIDIAN_EXPORT_PATH")"
mkdir -p "$OUTPUT_DIR"

RESPONSE=$(curl -sS -w "\n%{http_code}" \
  -H "Authorization: Bearer $OBSIDIAN_API_KEY" \
  "$OBSIDIAN_MAIN_API_URL$OBSIDIAN_PROJECT_INDEX_PATH")

HTTP_CODE=$(echo "$RESPONSE" | tail -1)
BODY=$(echo "$RESPONSE" | sed '$d')

if [ "$HTTP_CODE" != "200" ]; then
  echo "[export-obsidian-index] ERROR: API returned HTTP $HTTP_CODE"
  echo "$BODY"
  exit 1
fi

# Validate it parses as JSON
echo "$BODY" | python3 -m json.tool > /dev/null 2>&1 || {
  echo '[export-obsidian-index] ERROR: API response is not valid JSON'
  exit 1
}

echo "$BODY" > "$OBSIDIAN_EXPORT_PATH"
echo "[export-obsidian-index] OK: wrote $OBSIDIAN_EXPORT_PATH"
