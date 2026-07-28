#!/usr/bin/env bash
set -euo pipefail

# Stdio transports belong to the client session that created them. This guard
# makes every launch observable and removes only records for dead children; it
# deliberately never kills a live Codex/OpenCode MCP process.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE="${MEMORY_ENV_FILE:-$HOME/.config/memory-platform/memory.env}"
RELEASE_FILE="$ROOT/target/release/.memory-platform-release"
STATE_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/memory-platform"
REGISTRY_DIR="$STATE_DIR/mcp-transports"
LOG_FILE="$STATE_DIR/mcp-transport.log"

mkdir -p "$REGISTRY_DIR"
chmod 700 "$REGISTRY_DIR"

for record in "$REGISTRY_DIR"/*.pid; do
  [[ -e "$record" ]] || continue
  pid="$(basename "$record" .pid)"
  if ! kill -0 "$pid" 2>/dev/null; then
    rm -f "$record"
  fi
done

if [[ ! -r "$ENV_FILE" ]]; then
  printf '%s startup_failed reason=missing_environment\n' "$(date -u +%FT%TZ)" >> "$LOG_FILE"
  echo "memory MCP environment file is missing" >&2
  exit 78
fi
if [[ ! -x "$ROOT/target/release/mcp-server" || ! -r "$RELEASE_FILE" ]]; then
  printf '%s startup_failed reason=missing_release\n' "$(date -u +%FT%TZ)" >> "$LOG_FILE"
  echo "memory MCP release is not installed" >&2
  exit 78
fi

set -a
source "$ENV_FILE"
set +a
export MEMORY_BUILD_REVISION="$(cat "$RELEASE_FILE")"

record="$REGISTRY_DIR/$$.pid"
printf '%s\n' "$$" > "$record"
chmod 600 "$record"
printf '%s started pid=%s parent=%s revision=%s\n' \
  "$(date -u +%FT%TZ)" "$$" "$PPID" "$MEMORY_BUILD_REVISION" >> "$LOG_FILE"

set +e
"$ROOT/target/release/mcp-server" "$@"
status=$?
set -e
rm -f "$record"

if [[ "$status" == "0" ]]; then
  printf '%s stopped_cleanly pid=%s reason=stdio_peer_closed\n' \
    "$(date -u +%FT%TZ)" "$$" >> "$LOG_FILE"
else
  printf '%s stopped_with_error pid=%s exit=%s\n' \
    "$(date -u +%FT%TZ)" "$$" "$status" >> "$LOG_FILE"
fi
exit "$status"
