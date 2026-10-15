#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

# the release binary (built by scripts/setup.sh) needs no cargo on PATH, which a
# systemd user manager usually lacks
[[ -x target/release/ingest ]] && exec target/release/ingest "$@"
exec cargo run --quiet --bin ingest -- "$@"
