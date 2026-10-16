#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LAUNCH_AGENTS="$HOME/Library/LaunchAgents"
STATE_HOME="${XDG_STATE_HOME:-$HOME/.local/state}/memory-platform"
RUNNER="$ROOT/sync-to-neon.sh"

mkdir -p "$LAUNCH_AGENTS" "$STATE_HOME"
# Dashboard and full-recovery plists are owned by their dedicated installers.
for template in \
  "$ROOT/launchd/com.memory-platform.neon-sync.plist.template" \
  "$ROOT/launchd/com.memory-platform.neon-retry.plist.template" \
  "$ROOT/launchd/com.memory-platform.neon-count-audit.plist.template" \
  "$ROOT/launchd/com.memory-platform.neon-reconcile.plist.template" \
  "$ROOT/launchd/com.memory-platform.archive-verify.plist.template"; do
  name="$(basename "$template" .template)"
  target="$LAUNCH_AGENTS/$name"
  runner="$RUNNER"
  [[ "$name" == "com.memory-platform.neon-sync.plist" ]] && runner="$ROOT/scripts/run-memory-maintenance.sh"
  [[ "$name" == "com.memory-platform.neon-retry.plist" ]] && runner="$ROOT/scripts/run-memory-maintenance.sh"
  [[ "$name" == "com.memory-platform.archive-verify.plist" ]] && runner="$ROOT/scripts/verify-memory-archive.sh"
  sed -e "s|__RUNNER__|$runner|g" -e "s|__STATE_HOME__|$STATE_HOME|g" "$template" > "$target"
  launchctl bootout "gui/$(id -u)/${name%.plist}" 2>/dev/null || true
done

echo "Installed app-managed LaunchAgent definitions. Memory Platform starts them only while the app is open."
