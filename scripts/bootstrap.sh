#!/usr/bin/env bash
set -euo pipefail

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

echo "[bootstrap] rehydrating local store from Neon and live sources"
"$SCRIPT_DIR/rehydrate_local.sh"

echo "[bootstrap] verifying backup coverage"
python3 "$SCRIPT_DIR/verify_backups.py"
