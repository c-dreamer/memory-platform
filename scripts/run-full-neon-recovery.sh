#!/usr/bin/env bash
set -euo pipefail
export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:$PATH"

# Manual-only recovery controller. Each invocation uses the resumable worker,
# so a disconnect leaves the local outbox intact and the next attempt resumes.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE="${MEMORY_ENV_FILE:-$HOME/.config/memory-platform/memory.env}"
STATE_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/memory-platform"
LOG_FILE="${MEMORY_FULL_SYNC_LOG:-$STATE_DIR/manual-full-sync.log}"
LOCK_DIR="$STATE_DIR/manual-full-sync.lock"

[[ -r "$ENV_FILE" ]] || { echo "memory environment file is missing" >&2; exit 78; }
[[ -x "$ROOT/target/release/neon-sync" ]] || { echo "neon-sync release is not installed" >&2; exit 78; }
mkdir -p "$STATE_DIR"
if ! mkdir "$LOCK_DIR" 2>/dev/null; then
  echo "manual full recovery is already running" >&2
  exit 75
fi
trap 'rmdir "$LOCK_DIR"' EXIT

set -a
source "$ENV_FILE"
set +a

printf '%s manual full recovery started\n' "$(date -u +%FT%TZ)" >> "$LOG_FILE"
for attempt in {1..30}; do
  printf '%s attempt=%s\n' "$(date -u +%FT%TZ)" "$attempt" >> "$LOG_FILE"
  "$ROOT/target/release/neon-sync" full --confirm-full-push >> "$LOG_FILE" 2>&1 || true
  queue="$(psql "$DATABASE_URL" -Atqc 'SELECT count(*) FROM sync_meta.outbox')"
  printf '%s remaining_queue=%s\n' "$(date -u +%FT%TZ)" "$queue" >> "$LOG_FILE"
  if [[ "$queue" == "0" ]]; then
    "$ROOT/target/release/neon-sync" pull >> "$LOG_FILE" 2>&1 || true
    "$ROOT/target/release/neon-sync" status >> "$LOG_FILE" 2>&1 || true
    printf '%s manual full recovery completed\n' "$(date -u +%FT%TZ)" >> "$LOG_FILE"
    exit 0
  fi
  sleep 5
done

printf '%s manual full recovery stopped after bounded attempts\n' "$(date -u +%FT%TZ)" >> "$LOG_FILE"
exit 1
