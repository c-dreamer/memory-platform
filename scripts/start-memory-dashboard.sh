#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE="${MEMORY_ENV_FILE:-$HOME/.config/memory-platform/memory.env}"
TOKEN_FILE="${MEMORY_DASHBOARD_TOKEN_FILE:-$HOME/.config/memory-platform/dashboard.token}"
[[ -r "$ENV_FILE" ]] || { echo "memory dashboard environment file is missing" >&2; exit 78; }
[[ -x "$ROOT/target/release/memory-dashboard" ]] || { echo "memory dashboard release is not installed" >&2; exit 78; }
set -a
source "$ENV_FILE"
set +a
if [[ ! -s "$TOKEN_FILE" ]]; then
  umask 077
  mkdir -p "$(dirname "$TOKEN_FILE")"
  openssl rand -hex 32 > "$TOKEN_FILE"
fi
chmod 600 "$TOKEN_FILE"
export MEMORY_DASHBOARD_TOKEN="$(<"$TOKEN_FILE")"
export MEMORY_PLATFORM_ROOT="$ROOT"
exec "$ROOT/target/release/memory-dashboard"
