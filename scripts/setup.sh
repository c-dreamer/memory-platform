#!/usr/bin/env bash
set -euo pipefail

# One-shot setup for a fresh Linux/macOS checkout: build the release binaries,
# create the protected env file, and (Linux) install the systemd user units.
# Safe to re-run; it never overwrites an existing env file or touches the database.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE="${MEMORY_ENV_FILE:-$HOME/.config/memory-platform/memory.env}"

command -v cargo >/dev/null || { echo "cargo not found -- install Rust from https://rustup.rs" >&2; exit 1; }
command -v git >/dev/null || { echo "git not found" >&2; exit 1; }

if [[ ! -e "$ENV_FILE" ]]; then
  mkdir -p "$(dirname "$ENV_FILE")"
  chmod 700 "$(dirname "$ENV_FILE")"
  cp "$ROOT/.env.example" "$ENV_FILE"
  chmod 600 "$ENV_FILE"
  echo "Created $ENV_FILE from .env.example -- fill in DATABASE_URL and the rest before starting anything."
fi
# The MCP guard reads $ENV_FILE; the services and older scripts read <repo>/.env.
# Point the second at the first so there is one file to edit.
if [[ ! -e "$ROOT/.env" ]]; then
  ln -s "$ENV_FILE" "$ROOT/.env"
fi

"$ROOT/scripts/install-memory-release.sh"
(cd "$ROOT" && cargo build --release --features transport-http --bin memory-platform)

if [[ "$(uname)" == "Linux" ]] && command -v systemctl >/dev/null && systemctl --user show-environment >/dev/null 2>&1; then
  "$ROOT/scripts/install-systemd-user.sh"
elif [[ "$(uname)" == "Darwin" ]]; then
  echo "macOS: the background jobs are launchd agents -- see scripts/install-neon-sync-launchd.sh."
fi

echo
echo "Next: edit $ENV_FILE, then register the MCP server with your agents:"
echo "  $ROOT/scripts/install-mcp-clients.sh          # prints the commands"
echo "  $ROOT/scripts/install-mcp-clients.sh --apply  # runs them for the CLIs it finds"
