#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_DIR"

if [[ -f "$REPO_DIR/.env" ]]; then
  set -a
  # shellcheck disable=SC1091
  . "$REPO_DIR/.env"
  set +a
fi

LOCAL_URL="${LOCAL_URL:-${DATABASE_URL:-}}"
NEON_URL="${NEON_URL:-${NEON_DATABASE_URL:-}}"

if [[ -z "$LOCAL_URL" ]]; then
  echo "[rehydrate] ERROR: LOCAL_URL or DATABASE_URL must be set"
  exit 1
fi

if [[ -z "$NEON_URL" ]]; then
  echo "[rehydrate] ERROR: NEON_URL or NEON_DATABASE_URL must be set"
  exit 1
fi

echo "[rehydrate] restoring local store from Neon into: $LOCAL_URL"
docker run --rm \
  postgres:18 \
  pg_dump "$NEON_URL" --clean --if-exists --no-owner --no-acl \
  | sed '/pg_session_jwt/d' \
  | psql "$LOCAL_URL" -v ON_ERROR_STOP=1 >/dev/null

echo "[rehydrate] ingesting current local source files"
DATABASE_URL="$LOCAL_URL" cargo run --quiet --bin ingest -- all

echo "[rehydrate] local vs neon summary"
cargo run --quiet --bin stats -- --compare "$LOCAL_URL" "$NEON_URL"
