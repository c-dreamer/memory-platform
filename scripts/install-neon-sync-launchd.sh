#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LAUNCH_AGENTS="$HOME/Library/LaunchAgents"
STATE_HOME="${XDG_STATE_HOME:-$HOME/.local/state}/memory-platform"
RUNNER="$ROOT/sync-to-neon.sh"

mkdir -p "$LAUNCH_AGENTS" "$STATE_HOME"
for template in "$ROOT"/launchd/*.plist.template; do
  name="$(basename "$template" .template)"
  target="$LAUNCH_AGENTS/$name"
  runner="$RUNNER"
  [[ "$name" == "com.memory-platform.archive-verify.plist" ]] && runner="$ROOT/scripts/verify-memory-archive.sh"
  sed -e "s|__RUNNER__|$runner|g" -e "s|__STATE_HOME__|$STATE_HOME|g" "$template" > "$target"
  launchctl bootout "gui/$(id -u)/${name%.plist}" 2>/dev/null || true
  launchctl bootstrap "gui/$(id -u)" "$target"
done

echo "Installed resumable Neon sync LaunchAgents. Credentials remain in $ROOT/.env."
