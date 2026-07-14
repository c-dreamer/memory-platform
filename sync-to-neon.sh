#!/usr/bin/env bash
set -euo pipefail

# Compatibility entry point. Credentials remain in environment variables/.env,
# never in process arguments or state files.
ROOT="$(cd "$(dirname "$0")" && pwd)"
if [[ -f "$ROOT/.env" ]]; then
  set -a
  # shellcheck disable=SC1091
  . "$ROOT/.env"
  set +a
fi

case "${1:-run}" in
  run|reconcile|status|rebuild-derived)
    command="${1:-run}"
    shift || true
    exec cargo run --quiet --manifest-path "$ROOT/Cargo.toml" --bin neon-sync -- "$command" "$@"
    ;;
  --full|reset-target)
    echo "Full dumps are retired. Emergency reset requires: neon-sync reset-target --confirm-neon-reset" >&2
    exit 2
    ;;
  *)
    echo "Usage: $0 [run|reconcile|status|rebuild-derived]" >&2
    exit 2
    ;;
esac
