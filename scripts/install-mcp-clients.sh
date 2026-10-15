#!/usr/bin/env bash
set -euo pipefail

# Register the memory-platform MCP server with the AI agents installed here.
# Every client launches scripts/mcp-entrypoint.sh over stdio; secrets stay in the
# env file, never in client config. Default: print the commands. --apply: run
# them for each CLI found on PATH. Clients without an add command get a snippet.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENTRY="$ROOT/scripts/mcp-entrypoint.sh"
NAME="memory-platform"
APPLY=0
[[ "${1:-}" == "--apply" ]] && APPLY=1

run() {
  if (( APPLY )); then
    echo "+ $*"
    "$@" || echo "  (failed -- is $NAME already registered? remove it first to re-add)" >&2
  else
    printf ' %q' "$@"; echo
  fi
}

found=0
if command -v claude >/dev/null; then found=1; echo "Claude Code:"; run claude mcp add --transport stdio --scope user "$NAME" -- "$ENTRY"; fi
if command -v codex >/dev/null; then found=1; echo "Codex CLI:"; run codex mcp add "$NAME" -- "$ENTRY"; fi
if command -v gemini >/dev/null; then found=1; echo "Gemini CLI:"; run gemini mcp add -s user "$NAME" "$ENTRY"; fi
if command -v hermes >/dev/null; then found=1; echo "Hermes Agent:"; run hermes mcp add "$NAME" --command "$ENTRY"; fi
(( found )) || echo "No claude/codex/gemini/hermes CLI found on PATH."

cat <<SNIPPETS

OpenCode (~/.config/opencode/opencode.json):
  { "mcp": { "$NAME": { "type": "local", "command": ["$ENTRY"], "enabled": true } } }

Cursor (~/.cursor/mcp.json):
  { "mcpServers": { "$NAME": { "command": "$ENTRY" } } }
SNIPPETS
