#!/usr/bin/env bash
set -euo pipefail

# Compatibility entry point. Clients and scheduled jobs use the verified release
# and load credentials only from the protected per-device environment file.
ROOT="$(cd "$(dirname "$0")" && pwd)"
ENV_FILE="${MEMORY_ENV_FILE:-$HOME/.config/memory-platform/memory.env}"
BIN="$ROOT/target/release/neon-sync"
ACTION="${1:-push}"

# Reject retired and unknown commands before reading credentials. Status and
# health are the only commands allowed through the pause marker.
case "$ACTION" in
  status|health|run|push|pull|audit|reconcile|migrate|bootstrap|rebuild-derived) ;;
  --full|reset-target)
    echo "Full dumps are retired. Emergency reset requires: neon-sync reset-target --confirm-neon-reset" >&2
    exit 2
    ;;
  *)
    echo "Usage: $0 [health|status|migrate|bootstrap|push|pull|audit|rebuild-derived]" >&2
    exit 2
esac

pause_marker_state() {
  local pause_file="${XDG_STATE_HOME:-$HOME/.local/state}/memory-platform/neon-maintenance.paused"
  # lstat distinguishes a missing marker from an unreadable state path and
  # treats even a dangling marker symlink as paused. Fail closed if Perl cannot
  # inspect the path.
  if /usr/bin/perl -e 'my $p=shift; exit(lstat($p) ? 0 : ($!{ENOENT} ? 2 : 1))' "$pause_file"; then
    return 0
  else
    local status=$?
    [[ $status -eq 2 ]] && return 1
    echo "Cannot safely check memory sync pause state at $pause_file." >&2
    return 2
  fi
}

if [[ "$ACTION" != status && "$ACTION" != health ]]; then
  if pause_marker_state; then
    echo "Memory sync is paused by the local control panel; skipped $ACTION." >&2
    exit 0
  else
    pause_status=$?
    [[ $pause_status -eq 1 ]] || exit 78
  fi
fi

[[ -r "$ENV_FILE" ]] || { echo "memory sync environment file is missing: $ENV_FILE" >&2; exit 78; }
[[ -x "$BIN" && -r "$ROOT/target/release/.memory-platform-release" ]] || {
  echo "memory sync release is not installed; run scripts/install-memory-release.sh" >&2
  exit 78
}
set -a
# shellcheck disable=SC1090
source "$ENV_FILE"
set +a

if [[ "$ACTION" != status && "$ACTION" != health ]]; then
  if pause_marker_state; then
    echo "Memory sync is paused by the local control panel; skipped $ACTION." >&2
    exit 0
  else
    pause_status=$?
    [[ $pause_status -eq 1 ]] || exit 78
  fi
fi

command="$ACTION"
shift || true
exec "$BIN" "$command" "$@"
