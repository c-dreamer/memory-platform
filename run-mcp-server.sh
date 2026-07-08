#!/usr/bin/env bash
set -euo pipefail

cd /home/humanoracle26/memory-platform-rust

if [[ -f .env ]]; then
  set -a
  # shellcheck disable=SC1091
  . ./.env
  set +a
fi

export RUST_LOG="${MCP_RUST_LOG:-off}"

exec /home/humanoracle26/memory-platform-rust/target/release/mcp-server
