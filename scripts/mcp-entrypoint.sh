#!/usr/bin/env bash
set -euo pipefail

# Shared Codex/OpenCode entry point.  Client configuration contains this path
# only; secrets remain in the protected environment file.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
exec "$ROOT/scripts/mcp-transport-guard.sh" "$@"
