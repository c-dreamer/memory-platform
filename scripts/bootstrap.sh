#!/usr/bin/env bash
set -euo pipefail

# Rehydrating replaces the local store with Neon's projection, so it only runs
# when asked: --rehydrate (plus --confirm-overwrite-local if the local DB
# already holds data; see scripts/rehydrate_local.sh).
REHYDRATE=0
CONFIRM_OVERWRITE=""
for arg in "$@"; do
  case "$arg" in
    --rehydrate) REHYDRATE=1 ;;
    --confirm-overwrite-local) CONFIRM_OVERWRITE="$arg" ;;
    *) echo "Usage: $0 [--rehydrate [--confirm-overwrite-local]]" >&2; exit 2 ;;
  esac
done
if [[ -n "$CONFIRM_OVERWRITE" && "$REHYDRATE" != 1 ]]; then
  echo "--confirm-overwrite-local only applies with --rehydrate" >&2
  exit 2
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_DIR"

if [[ -f "$REPO_DIR/.env" ]]; then
  set -a
  # shellcheck disable=SC1091
  . "$REPO_DIR/.env"
  set +a
fi

echo "[bootstrap] building memory platform binaries"
cargo build --release --features transport-http --bin memory-platform --bin mcp-server --bin ingest --bin stats

echo "[bootstrap] checking tracked files for secret leaks"
"$SCRIPT_DIR/check_secrets.sh"

if [[ "$REHYDRATE" == 1 ]]; then
  echo "[bootstrap] rehydrating local store from Neon and live sources"
  "$SCRIPT_DIR/rehydrate_local.sh" ${CONFIRM_OVERWRITE:+"$CONFIRM_OVERWRITE"}
else
  echo "[bootstrap] skipping rehydrate (pass --rehydrate to restore an empty local store from Neon)"
fi

echo "[bootstrap] verifying backup coverage"
PY=python3
python3 --version >/dev/null 2>&1 || PY="py -3"
$PY "$SCRIPT_DIR/verify_backups.py"
