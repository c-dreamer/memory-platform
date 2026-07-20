#!/usr/bin/env bash
set -euo pipefail

# Low-priority, resumable maintenance. Every step is idempotent; an outage
# leaves durable local work for the next scheduled retry.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE="${MEMORY_ENV_FILE:-$HOME/.config/memory-platform/memory.env}"
INGEST="$ROOT/target/release/ingest"
STATE_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/memory-platform"
RETRY_FILE="$STATE_DIR/neon-maintenance.retry"
PAUSE_FILE="$STATE_DIR/neon-maintenance.paused"
MODE="${1:---daily}"

notify() {
  osascript -e "display notification \"$1\" with title \"Memory Platform\"" 2>/dev/null || true
}

[[ -r "$ENV_FILE" ]] || { echo "memory environment file is missing: $ENV_FILE" >&2; exit 78; }
[[ -x "$INGEST" ]] || { echo "memory ingest release is not installed" >&2; exit 78; }
mkdir -p "$STATE_DIR"
if [[ -e "$PAUSE_FILE" ]]; then
  echo "Memory maintenance is paused by the local control panel."
  exit 0
fi
if [[ "$MODE" == "--retry-only" && ! -e "$RETRY_FILE" ]]; then
  exit 0
fi
set -a
# shellcheck disable=SC1090
source "$ENV_FILE"
set +a

if [[ "$MODE" == "--retry-only" ]]; then
  notify "Retrying the previously failed bounded sync: up to 10 minutes, 25 rows or 2 MiB per transaction."
else
  notify "Daily ingestion starting: settled local Codex sessions and pending Neon changes; up to 10 minutes, 25 rows or 2 MiB per transaction."
fi

on_failure() {
  local status=$?
  touch "$RETRY_FILE"
  notify "Maintenance paused after a failure. It will retry on the next hourly network check; no data was discarded."
  exit "$status"
}
trap on_failure ERR

# Current Codex archives are ingested idempotently. The importer keeps source
# identities, so reruns only update changed, settled sessions.
"$INGEST" codex
"$ROOT/sync-to-neon.sh" bootstrap
"$ROOT/sync-to-neon.sh" push
"$ROOT/sync-to-neon.sh" pull
rm -f "$RETRY_FILE"
notify "Maintenance completed. Session ingestion and bounded Neon sync finished."
