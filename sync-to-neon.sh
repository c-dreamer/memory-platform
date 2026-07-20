#!/usr/bin/env bash
set -euo pipefail

# Compatibility entry point. Clients and scheduled jobs use the verified release
# and load credentials only from the protected per-device environment file.
ROOT="$(cd "$(dirname "$0")" && pwd)"
ENV_FILE="${MEMORY_ENV_FILE:-$HOME/.config/memory-platform/memory.env}"
BIN="$ROOT/target/release/neon-sync"

[[ -r "$ENV_FILE" ]] || { echo "memory sync environment file is missing: $ENV_FILE" >&2; exit 78; }
[[ -x "$BIN" && -r "$ROOT/target/release/.memory-platform-release" ]] || {
  echo "memory sync release is not installed; run scripts/install-memory-release.sh" >&2
  exit 78
}
set -a
# shellcheck disable=SC1090
source "$ENV_FILE"
set +a

case "${1:-push}" in
  run|push|pull|audit|reconcile|status|health|migrate|bootstrap|rebuild-derived)
    command="${1:-push}"
    shift || true
    exec "$BIN" "$command" "$@"
    ;;
  --full|reset-target)
    echo "Full dumps are retired. Emergency reset requires: neon-sync reset-target --confirm-neon-reset" >&2
    exit 2
    ;;
  *)
    echo "Usage: $0 [health|status|migrate|bootstrap|push|pull|audit|rebuild-derived]" >&2
    exit 2
    ;;
esac
