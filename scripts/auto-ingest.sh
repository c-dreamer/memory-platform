#!/usr/bin/env bash
# Autonomous session ingestion — runs periodically via LaunchAgent to pick up
# new OpenCode, Codex and Hermes sessions without re-scanning the heavy vault.
set -euo pipefail

REPO_ROOT="${MEMORY_PLATFORM_ROOT:-/Users/yahwehatwork/Documents/AI/Github Repos/memory-platform}"
ROOT="$REPO_ROOT"
BIN="$ROOT/target/release/ingest"

# Ensure the binary exists
if [[ ! -x "$BIN" ]]; then
  echo "ingest binary not found at $BIN; building..." >&2
  (cd "$ROOT" && cargo build --release --bin ingest) >&2
fi

# Load credentials from .env (never printed)
if [[ -f "$ROOT/.env" ]]; then
  set -a
  # shellcheck disable=SC1091
  source "$ROOT/.env"
  set +a
fi

if [[ -z "${DATABASE_URL:-}" ]]; then
  echo "DATABASE_URL not set" >&2
  exit 1
fi

"$BIN" --db-url "$DATABASE_URL" \
  sessions \
  --source "$HOME/.local/share/opencode/opencode.db" \
  2>&1 | grep -E "Session ingestion complete|error" || true

"$BIN" --db-url "$DATABASE_URL" \
  codex \
  --path "$HOME/.codex/sessions" \
  2>&1 | grep -E "Codex|complete|error" || true

"$BIN" --db-url "$DATABASE_URL" \
  hermes \
  --source "$HOME/.hermes/state.db" \
  2>&1 | grep -E "Session ingestion complete|error" || true

"$BIN" --db-url "$DATABASE_URL" \
  config \
  --dir "$HOME/.config/opencode" \
  2>&1 | grep -E "complete|error" || true

echo "auto-ingest finished at $(date '+%Y-%m-%d %H:%M:%S')"