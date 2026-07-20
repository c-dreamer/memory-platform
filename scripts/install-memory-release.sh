#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
cargo build --release --bin mcp-server --bin neon-sync --bin ingest --bin memory-dashboard
git rev-parse HEAD > target/release/.memory-platform-release
chmod 600 target/release/.memory-platform-release
echo "Installed memory-platform release $(cat target/release/.memory-platform-release)"
