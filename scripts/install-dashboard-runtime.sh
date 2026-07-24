#!/usr/bin/env bash
set -euo pipefail

# Keep the app's local API independent from a disposable source checkout.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REVISION="$(git -C "$ROOT" rev-parse HEAD)"
RUNTIME_ROOT="${MEMORY_RUNTIME_ROOT:-$HOME/Library/Application Support/Memory Platform/runtime/$REVISION}"
STATE_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/memory-platform"
PLIST="$HOME/Library/LaunchAgents/com.memory-platform.dashboard.plist"

[[ -x "$ROOT/target/release/memory-dashboard" ]] || {
  echo "build the memory-dashboard release before installing the runtime" >&2
  exit 78
}

install -d "$RUNTIME_ROOT/target/release" "$RUNTIME_ROOT/scripts" "$STATE_DIR" "$HOME/Library/LaunchAgents"
install -m 755 "$ROOT/target/release/memory-dashboard" "$RUNTIME_ROOT/target/release/memory-dashboard"
install -m 755 "$ROOT/scripts/start-memory-dashboard.sh" "$RUNTIME_ROOT/scripts/start-memory-dashboard.sh"
printf '%s\n' "$REVISION" > "$RUNTIME_ROOT/target/release/.memory-platform-release"
chmod 600 "$RUNTIME_ROOT/target/release/.memory-platform-release"

sed -e "s|__RUNNER__|$RUNTIME_ROOT/scripts/start-memory-dashboard.sh|g" \
    -e "s|__STATE_HOME__|$STATE_DIR|g" \
    "$ROOT/launchd/com.memory-platform.dashboard.plist.template" > "$PLIST"
plutil -lint "$PLIST" >/dev/null

echo "Installed dashboard runtime: $RUNTIME_ROOT"
echo "LaunchAgent definition: $PLIST"
