#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LAUNCH_AGENTS="$HOME/Library/LaunchAgents"
STATE_HOME="$HOME/.local/state/memory-platform"
RUNNER="$ROOT/sync-to-neon.sh"

# Model probe/router automation is OPT-IN (see AGENTS.md "Model Router Automation").
# Default install skips it; pass --with-model-router to enable, --remove-model-router to uninstall.
WITH_MODEL_ROUTER=0
REMOVE_MODEL_ROUTER=0
for arg in "$@"; do
  case "$arg" in
    --with-model-router) WITH_MODEL_ROUTER=1 ;;
    --remove-model-router) REMOVE_MODEL_ROUTER=1 ;;
    *) echo "Unknown option: $arg" >&2; exit 1 ;;
  esac
done

# macOS TCC blocks launchd from executing scripts under ~/Documents, so the
# memory-platform runner scripts are staged into the state dir (non-protected).
RUNNERS_DIR="$STATE_HOME/runners"
mkdir -p "$LAUNCH_AGENTS" "$STATE_HOME" "$RUNNERS_DIR"

stage_runner() {
  local script="$1"
  local dest="$RUNNERS_DIR/$(basename "$script")"
  # Point the staged copy at the repo root for binary/.env resolution.
  sed -e "s|MEMORY_PLATFORM_ROOT=.*|MEMORY_PLATFORM_ROOT=$ROOT|" "$script" > "$dest"
  chmod +x "$dest"
  echo "$dest"
}

if [[ "$REMOVE_MODEL_ROUTER" == 1 ]]; then
  for name in com.memory-platform.model-probe.plist com.memory-platform.model-router.plist; do
    launchctl bootout "gui/$(id -u)/${name%.plist}" 2>/dev/null || true
    rm -f "$LAUNCH_AGENTS/$name"
  done
  rm -f "$RUNNERS_DIR/model-probe.sh" "$RUNNERS_DIR/model-router.sh"
  echo "Removed model-probe + model-router LaunchAgents."
  exit 0
fi

for template in "$ROOT"/launchd/*.plist.template; do
  name="$(basename "$template" .template)"
  # Skip opt-in agents unless explicitly requested.
  if [[ "$name" == com.memory-platform.model-* && "$WITH_MODEL_ROUTER" != 1 ]]; then
    continue
  fi
  target="$LAUNCH_AGENTS/$name"
  runner="$RUNNER"
  [[ "$name" == "com.memory-platform.archive-verify.plist" ]] && runner="$ROOT/scripts/verify-memory-archive.sh"
  [[ "$name" == "com.memory-platform.ingest.plist" ]] && runner="$(stage_runner "$ROOT/scripts/auto-ingest.sh")"
  [[ "$name" == "com.memory-platform.model-probe.plist" ]] && runner="$(stage_runner "$ROOT/scripts/model-probe.sh")"
  [[ "$name" == "com.memory-platform.model-router.plist" ]] && runner="$(stage_runner "$ROOT/scripts/model-router.sh")"
  sed -e "s|__RUNNER__|$runner|g" -e "s|__STATE_HOME__|$STATE_HOME|g" "$template" > "$target"
  launchctl bootout "gui/$(id -u)/${name%.plist}" 2>/dev/null || true
  launchctl bootstrap "gui/$(id -u)" "$target"
done

echo "Installed LaunchAgents (runners staged in $RUNNERS_DIR). Credentials remain in $ROOT/.env."
[[ "$WITH_MODEL_ROUTER" == 1 ]] || echo "Model probe/router skipped (opt-in: --with-model-router)."
