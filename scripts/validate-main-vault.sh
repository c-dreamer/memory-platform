#!/usr/bin/env bash
set -euo pipefail

ROOT="${MEMORY_MAIN_VAULT_ROOT:?MEMORY_MAIN_VAULT_ROOT is required}"
case "$ROOT" in
  */Documents/Main\ Vault) ;;
  *) echo "main vault root must be the dedicated Documents/Main Vault directory" >&2; exit 2 ;;
esac
[[ -d "$ROOT" ]] || { echo "main vault root does not exist: $ROOT" >&2; exit 2; }
for forbidden in "$ROOT/Law Modules Vault" "$ROOT/.obsidian"; do
  [[ ! -e "$forbidden" ]] || { echo "forbidden nested content in main vault root: $forbidden" >&2; exit 2; }
done
echo "main vault boundary is valid: $ROOT"
