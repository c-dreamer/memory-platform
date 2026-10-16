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

pause_marker_state() {
  local pause_file="${XDG_STATE_HOME:-$HOME/.local/state}/memory-platform/neon-maintenance.paused"
  if /usr/bin/perl -e 'my $p=shift; exit(lstat($p) ? 0 : ($!{ENOENT} ? 2 : 1))' "$pause_file"; then
    return 0
  else
    local status=$?
    [[ $status -eq 2 ]] && return 1
    echo "Cannot safely check memory sync pause state at $pause_file." >&2
    return 2
  fi
}

stop_if_paused() {
  if pause_marker_state; then
    echo "Memory sync is paused by the local control panel; full recovery was skipped." >&2
    exit 0
  else
    local status=$?
    [[ $status -eq 1 ]] || exit 78
  fi
}

stop_if_paused

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

# neon-sync (below) reads DATABASE_URL from its own inherited environment and
# needs the password embedded in it (sqlx has no PGPASSWORD fallback). The
# direct `psql` call below gets a password-free URL argv plus PGPASSWORD.
. "$ROOT/scripts/lib/pg-env.sh"

run_required() {
  local status=0
  "$@" >> "$LOG_FILE" 2>&1 || status=$?
  if (( status != 0 )); then
    stop_if_paused
    printf '%s required_command_failed status=%s command=%q\n' \
      "$(date -u +%FT%TZ)" "$status" "$*" >> "$LOG_FILE"
    return "$status"
  fi
}

printf '%s manual full recovery started\n' "$(date -u +%FT%TZ)" >> "$LOG_FILE"
for attempt in {1..30}; do
  stop_if_paused
  printf '%s attempt=%s\n' "$(date -u +%FT%TZ)" "$attempt" >> "$LOG_FILE"
  run_required "$ROOT/target/release/neon-sync" full --confirm-full-push
  stop_if_paused
  queue="$(PGPASSWORD="$(pg_password "$DATABASE_URL")" psql "$(pg_url_strip_password "$DATABASE_URL")" -Atqc 'SELECT count(*) FROM sync_meta.outbox')"
  pending_events="$(PGPASSWORD="$(pg_password "$DATABASE_URL")" psql "$(pg_url_strip_password "$DATABASE_URL")" -Atqc 'SELECT count(*) FROM sync_meta.events WHERE pushed_at IS NULL')"
  printf '%s remaining_queue=%s unpublished_events=%s\n' \
    "$(date -u +%FT%TZ)" "$queue" "$pending_events" >> "$LOG_FILE"
  if [[ "$queue" == "0" && "$pending_events" == "0" ]]; then
    stop_if_paused
    run_required "$ROOT/target/release/neon-sync" pull
    stop_if_paused
    run_required "$ROOT/target/release/neon-sync" status
    stop_if_paused
    queue="$(PGPASSWORD="$(pg_password "$DATABASE_URL")" psql "$(pg_url_strip_password "$DATABASE_URL")" -Atqc 'SELECT count(*) FROM sync_meta.outbox')"
    pending_events="$(PGPASSWORD="$(pg_password "$DATABASE_URL")" psql "$(pg_url_strip_password "$DATABASE_URL")" -Atqc 'SELECT count(*) FROM sync_meta.events WHERE pushed_at IS NULL')"
    if [[ "$queue" == "0" && "$pending_events" == "0" ]]; then
      stop_if_paused
      printf '%s sync queues drained; verify the recovery parity checklist before declaring recovery complete\n' \
        "$(date -u +%FT%TZ)" >> "$LOG_FILE"
      exit 0
    fi
    printf '%s post_pull_queue=%s post_pull_unpublished_events=%s; continuing\n' \
      "$(date -u +%FT%TZ)" "$queue" "$pending_events" >> "$LOG_FILE"
  fi
  sleep 5
done

printf '%s manual full recovery stopped after bounded attempts\n' "$(date -u +%FT%TZ)" >> "$LOG_FILE"
exit 1
