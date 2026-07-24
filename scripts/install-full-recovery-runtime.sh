#!/usr/bin/env bash
set -euo pipefail

# Install the explicit manual recovery worker outside Documents so macOS
# launchd can execute it without TCC blocking the user's source checkout.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REVISION="$(git -C "$ROOT" rev-parse HEAD)"
RUNTIME_ROOT="${MEMORY_RUNTIME_ROOT:-$HOME/Library/Application Support/Memory Platform/runtime/$REVISION}"
STATE_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/memory-platform"
PLIST="$HOME/Library/LaunchAgents/com.memory-platform.full-recovery.plist"

[[ -x "$ROOT/target/release/neon-sync" ]] || {
  echo "build the neon-sync release before installing the runtime" >&2
  exit 78
}

install -d "$RUNTIME_ROOT/target/release" "$RUNTIME_ROOT/scripts" "$STATE_DIR" "$HOME/Library/LaunchAgents"
install -m 755 "$ROOT/target/release/neon-sync" "$RUNTIME_ROOT/target/release/neon-sync"
install -m 755 "$ROOT/scripts/run-full-neon-recovery.sh" "$RUNTIME_ROOT/scripts/run-full-neon-recovery.sh"
printf '%s\n' "$REVISION" > "$RUNTIME_ROOT/target/release/.memory-platform-release"
chmod 600 "$RUNTIME_ROOT/target/release/.memory-platform-release"

sed -e "s|__RUNNER__|$RUNTIME_ROOT/scripts/run-full-neon-recovery.sh|g" \
    -e "s|__HOME__|$HOME|g" \
    -e "s|__STATE__|$STATE_DIR|g" \
    "$ROOT/launchd/com.memory-platform.full-recovery.plist.template" > "$PLIST"
plutil -lint "$PLIST" >/dev/null

echo "Installed manual recovery runtime: $RUNTIME_ROOT"
echo "LaunchAgent definition: $PLIST"
