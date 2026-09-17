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

# cargo run (below) needs the full URLs with password embedded (sqlx has no
# PGPASSWORD fallback), so LOCAL_URL/NEON_URL stay intact for that; only the
# pg_dump/psql argv below gets password-free URLs plus PGPASSWORD.
. "$SCRIPT_DIR/lib/pg-env.sh"
NEON_PW="$(pg_password "$NEON_URL")"
LOCAL_PW="$(pg_password "$LOCAL_URL")"
NEON_URL_NOPASS="$(pg_url_strip_password "$NEON_URL")"
LOCAL_URL_NOPASS="$(pg_url_strip_password "$LOCAL_URL")"

echo "[rehydrate] restoring local store from Neon into: $LOCAL_URL_NOPASS"
PGPASSWORD="$NEON_PW" docker run --rm -e PGPASSWORD \
  pgvector/pgvector:pg17 \
  pg_dump "$NEON_URL_NOPASS" --clean --if-exists --no-owner --no-acl \
  | sed '/pg_session_jwt/d' \
  | PGPASSWORD="$LOCAL_PW" psql "$LOCAL_URL_NOPASS" -v ON_ERROR_STOP=1 >/dev/null

echo "[rehydrate] ingesting current local source files"
DATABASE_URL="$LOCAL_URL" cargo run --quiet --bin ingest -- all

echo "[rehydrate] local vs neon summary"
cargo run --quiet --bin stats -- --compare "$LOCAL_URL" "$NEON_URL"
