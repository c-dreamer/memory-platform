#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if [[ "$(uname)" == "Darwin" ]]; then
  agents="$HOME/Library/LaunchAgents"
  state="${XDG_STATE_HOME:-$HOME/.local/state}/memory-platform"
  mkdir -p "$agents" "$state"
  target="$agents/com.memory-platform.dashboard.plist"
  sed -e "s|__RUNNER__|$ROOT/scripts/start-memory-dashboard.sh|g" -e "s|__STATE_HOME__|$state|g" "$ROOT/launchd/com.memory-platform.dashboard.plist.template" > "$target"
  launchctl bootout "gui/$(id -u)/com.memory-platform.dashboard" 2>/dev/null || true
  launchctl bootstrap "gui/$(id -u)" "$target"
else
  units="$HOME/.config/systemd/user"
  mkdir -p "$units"
  sed "s|__ROOT__|$ROOT|g" "$ROOT/systemd/memory-platform-dashboard.service.template" > "$units/memory-platform-dashboard.service"
  systemctl --user daemon-reload
  systemctl --user enable --now memory-platform-dashboard.service
fi
echo "Dashboard available locally at http://127.0.0.1:8765"
