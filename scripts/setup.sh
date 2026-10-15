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
  dir="$(dirname "$ENV_FILE")"
  [[ -d "$dir" ]] || { mkdir -p "$dir"; chmod 700 "$dir"; }
  # an existing <repo>/.env (the old README's `cp .env.example .env`) holds the real values
  if [[ -f "$ROOT/.env" && ! -L "$ROOT/.env" ]]; then src="$ROOT/.env"; else src="$ROOT/.env.example"; fi
  cp "$src" "$ENV_FILE"
  chmod 600 "$ENV_FILE"
  echo "Created $ENV_FILE from ${src#"$ROOT"/} -- check DATABASE_URL and the rest before starting anything."
fi
# The MCP guard reads $ENV_FILE; the services and older scripts read <repo>/.env.
# Point the second at the first so there is one file to edit.
if [[ -L "$ROOT/.env" && ! -e "$ROOT/.env" ]]; then
  ln -sfn "$ENV_FILE" "$ROOT/.env"  # dangling link, e.g. the env file moved
elif [[ ! -e "$ROOT/.env" ]]; then
  ln -s "$ENV_FILE" "$ROOT/.env"
elif [[ ! "$ROOT/.env" -ef "$ENV_FILE" ]]; then
  echo "warning: $ROOT/.env is not $ENV_FILE -- the services read the first, the MCP server the second; merge them and make .env a symlink" >&2
fi
if [[ -n "${MEMORY_ENV_FILE:-}" ]]; then
  echo "note: MCP clients launch with their own environment -- set MEMORY_ENV_FILE=$ENV_FILE in each client's env too" >&2
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
