#!/usr/bin/env bash
set -u

# Resumable, externally-pollable embedding repair. Each invocation of the Rust
# repair binary only selects rows that are still null, so a retry never resets
# or duplicates already repaired data.
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STATE_DIR="${MEMORY_PLATFORM_STATE_DIR:-$HOME/.local/state/memory-platform}"
STATUS_FILE="$STATE_DIR/embedding-repair-status.json"
LOG_FILE="$STATE_DIR/embedding-repair.log"
LOCK_FILE="$STATE_DIR/embedding-repair.lock"
MAX_ATTEMPTS="${EMBEDDING_REPAIR_ATTEMPTS:-5}"
RETRY_DELAY="${EMBEDDING_REPAIR_RETRY_DELAY:-60}"

mkdir -p "$STATE_DIR"

write_status() {
  local status="$1" attempt="$2" message="$3"
  local tmp="$STATUS_FILE.tmp.$$"
  printf '{"status":"%s","attempt":%s,"pid":%s,"updated_at":"%s","message":"%s"}\n' \
    "$status" "$attempt" "$$" "$(date -u +%FT%TZ)" "$(printf '%s' "$message" | sed 's/\\/\\\\/g; s/"/\\"/g')" \
    >"$tmp"
  mv -f "$tmp" "$STATUS_FILE"
}

status() {
  if [[ -f "$STATUS_FILE" ]]; then
    local raw pid current
    raw="$(cat "$STATUS_FILE")"
    pid="$(printf '%s' "$raw" | sed -n 's/.*"pid":\([0-9][0-9]*\).*/\1/p')"
    current="$(printf '%s' "$raw" | sed -n 's/.*"status":"\([^"]*\)".*/\1/p')"
    if [[ "$current" == "running" || "$current" == "retrying" ]] && [[ -n "$pid" ]] && ! kill -0 "$pid" 2>/dev/null; then
      printf '{"status":"failed","attempt":0,"pid":%s,"updated_at":"%s","message":"worker exited without recording completion; retry is safe"}\n' "$pid" "$(date -u +%FT%TZ)"
    else
      printf '%s\n' "$raw"
    fi
  else
    printf '{"status":"never_started","attempt":0,"message":"no repair has been started"}\n'
  fi
}

if [[ "${1:-start}" == "status" ]]; then
  status
  exit 0
fi

if [[ "${1:-start}" != "start" && "${1:-start}" != "retry" ]]; then
  echo "usage: $0 {start|retry|status}" >&2
  exit 2
fi

exec 9>"$LOCK_FILE"
if ! flock -n 9; then
  write_status running 0 "another repair worker already owns the lock"
  exit 0
fi

cd "$ROOT_DIR"
write_status running 0 "started"

for attempt in $(seq 1 "$MAX_ATTEMPTS"); do
  write_status running "$attempt" "repair attempt started"
  if set -a; [ -f .env ] && . ./.env; set +a; cargo run --quiet --bin repair_embeddings >>"$LOG_FILE" 2>&1; then
    write_status succeeded "$attempt" "all embedding tables verified"
    exit 0
  fi

  if [ "$attempt" -lt "$MAX_ATTEMPTS" ]; then
    write_status retrying "$attempt" "repair failed; waiting before resumable retry"
    sleep "$RETRY_DELAY"
  fi
done

write_status failed "$MAX_ATTEMPTS" "repair failed after maximum attempts; inspect embedding-repair.log"
exit 1
