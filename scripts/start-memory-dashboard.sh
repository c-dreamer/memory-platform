#!/usr/bin/env bash
set -euo pipefail
# Prepend, not replace: launchd gives a minimal PATH (Homebrew must come
# first), but replacing it dropped C:\Windows\System32 under Git Bash.
export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin${PATH:+:$PATH}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE="${MEMORY_ENV_FILE:-$HOME/.config/memory-platform/memory.env}"
TOKEN_FILE="${MEMORY_DASHBOARD_TOKEN_FILE:-$HOME/.config/memory-platform/dashboard.token}"
[[ -r "$ENV_FILE" ]] || { echo "memory dashboard environment file is missing" >&2; exit 78; }
BIN="$ROOT/target/release/memory-dashboard"
[[ -x "$BIN" ]] || BIN="$BIN.exe"  # Windows build
[[ -x "$BIN" ]] || { echo "memory dashboard release is not installed" >&2; exit 78; }
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
exec "$BIN"
