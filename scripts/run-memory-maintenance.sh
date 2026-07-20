#!/usr/bin/env bash
set -euo pipefail

# Low-priority, resumable maintenance. Every step is idempotent; an outage
# leaves durable local work for the next scheduled run.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE="${MEMORY_ENV_FILE:-$HOME/.config/memory-platform/memory.env}"
INGEST="$ROOT/target/release/ingest"

[[ -r "$ENV_FILE" ]] || { echo "memory environment file is missing: $ENV_FILE" >&2; exit 78; }
[[ -x "$INGEST" ]] || { echo "memory ingest release is not installed" >&2; exit 78; }
set -a
# shellcheck disable=SC1090
source "$ENV_FILE"
set +a

# Current Codex archives are ingested idempotently. The importer keeps source
# identities, so reruns only update changed, settled sessions.
"$INGEST" codex
"$ROOT/sync-to-neon.sh" bootstrap
"$ROOT/sync-to-neon.sh" push
"$ROOT/sync-to-neon.sh" pull
