#!/usr/bin/env bash
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_DIR"

patterns='(ghp_[A-Za-z0-9]{20,}|sk-or-v1-[A-Za-z0-9]{20,}|nvapi-[A-Za-z0-9]{20,}|AIza[0-9A-Za-z_-]{20,}|gsk_[A-Za-z0-9]{20,}|sb_(publishable|secret)_[A-Za-z0-9_]{10,}|npg_[A-Za-z0-9]{20,})'

# mktemp (not a fixed name) plus a private mode and a cleanup trap: a fixed,
# world-readable /tmp path would leave any matched secret readable by every
# other local user, indefinitely, on a shared host.
SCAN_OUT="$(mktemp)"
trap 'rm -f "$SCAN_OUT"' EXIT
chmod 600 "$SCAN_OUT"

if git grep -nE "$patterns" -- . \
  ':(exclude).env' \
  ':(exclude).env.example' \
  ':(exclude)Cargo.lock' \
  ':(exclude)target' \
  ':(exclude)obsidian-vault' \
  ':(exclude)thinclient_drives' >"$SCAN_OUT"; then
  echo "[secret-scan] potential secrets found in tracked files:"
  cat "$SCAN_OUT"
  exit 1
fi

echo "[secret-scan] no high-confidence secrets found in tracked files"
