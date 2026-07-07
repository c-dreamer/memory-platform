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

LOCAL_URL="${LOCAL_URL:-${DATABASE_URL:-postgresql://memory:YAft44tZyrG4DET0WeigY8BpZ%2BcqGgPtTXsPK4XFgXc%3D@127.0.0.1:5433/memory}}"
NEON_URL="${NEON_URL:-${NEON_DATABASE_URL:-postgres://neondb_owner:npg_Z38efMRscSXT@ep-mute-wind-at1qqxxb.c-9.us-east-1.aws.neon.tech/neondb?sslmode=require}}"

echo "[rehydrate] restoring local store from Neon into: $LOCAL_URL"
docker run --rm \
  -e PGPASSWORD="${NEON_PASSWORD:-}" \
  postgres:18 \
  pg_dump "$NEON_URL" --clean --if-exists --no-owner --no-acl \
  | psql "$LOCAL_URL" -v ON_ERROR_STOP=1 >/dev/null

echo "[rehydrate] ingesting current local source files"
DATABASE_URL="$LOCAL_URL" cargo run --quiet --bin ingest -- all

echo "[rehydrate] local vs neon summary"
cargo run --quiet --bin stats -- --compare "$LOCAL_URL" "$NEON_URL"
