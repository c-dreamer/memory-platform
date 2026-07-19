#!/usr/bin/env bash
set -euo pipefail

# Shared Codex/OpenCode entry point.  Client configuration contains this path
# only; secrets remain in the protected environment file.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE="${MEMORY_ENV_FILE:-$HOME/.config/memory-platform/memory.env}"
RELEASE_FILE="$ROOT/target/release/.memory-platform-release"

if [[ ! -r "$ENV_FILE" ]]; then
  echo "memory MCP environment file is missing: $ENV_FILE" >&2
  exit 78
fi
if [[ ! -x "$ROOT/target/release/mcp-server" || ! -r "$RELEASE_FILE" ]]; then
  echo "memory MCP release is not installed; run scripts/install-memory-release.sh" >&2
  exit 78
fi

set -a
source "$ENV_FILE"
set +a
export MEMORY_BUILD_REVISION="$(cat "$RELEASE_FILE")"
exec "$ROOT/target/release/mcp-server"
