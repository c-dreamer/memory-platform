#!/usr/bin/env bash
set -euo pipefail

# Export memory platform aggregates for Portfolio Radar's Memory MCP adapter.
# Writes a pre-redacted JSON export of project-level metrics.
# Usage: export-portfolio-mcp.sh [output-path]
# Defaults to $MEMORY_MCP_PORTFOLIO_EXPORT or /tmp/memory-mcp-portfolio-export.json

OUTPUT="${1:-${MEMORY_MCP_PORTFOLIO_EXPORT:-/tmp/memory-mcp-portfolio-export.json}}"

# Load env for DB credentials
if [[ -f "$(dirname "$0")/.env" ]]; then
  set -a; . "$(dirname "$0")/.env"; set +a
fi

PGHOST="${PGHOST:-127.0.0.1}"
PGPORT="${PGPORT:-5433}"
PGUSER="${PGUSER:-memory}"
PGPASSWORD="${PGPASSWORD:-password}"
PGDATABASE="${PGDATABASE:-memory}"

PSQL="psql -h $PGHOST -p $PGPORT -U $PGUSER -d $PGDATABASE -t -A"

echo "{\"generatedAt\":\"$(date -Iseconds)\",\"schemaVersion\":\"1.0\"," > "$OUTPUT"

# Table row counts
echo -n '"tableCounts":{' >> "$OUTPUT"
FIRST=true
for table in agents code_changes contradictions config documents embeddings experiences memories procedures projects relationships session_documents session_memories sessions summaries trading_results; do
  count=$($PSQL -c "SELECT count(*) FROM $table" 2>/dev/null || echo "0")
  $FIRST || echo -n ',' >> "$OUTPUT"
  FIRST=false
  echo -n "\"$table\":$count" >> "$OUTPUT"
done
echo '},' >> "$OUTPUT"

# Embedding coverage
echo -n '"embeddingCoverage":{' >> "$OUTPUT"
for table in documents memories experiences sessions; do
  total=$($PSQL -c "SELECT count(*) FROM $table" 2>/dev/null || echo "0")
  with_emb=$($PSQL -c "SELECT count(*) FROM $table WHERE embedding IS NOT NULL" 2>/dev/null || echo "0")
  echo -n "\"$table\":{\"total\":$total,\"withEmbedding\":$with_emb}," >> "$OUTPUT"
done
echo -n '"dummy":0},' >> "$OUTPUT"

# DB size
db_size=$($PSQL -c "SELECT pg_size_pretty(pg_database_size('memory'))" 2>/dev/null || echo "unknown")
echo -n "\"dbSize\":\"$db_size\"," >> "$OUTPUT"

# Recent activity
latest_session=$($PSQL -c "SELECT COALESCE(MAX(started_at)::text, 'never') FROM sessions" 2>/dev/null || echo "never")
latest_doc=$($PSQL -c "SELECT COALESCE(MAX(created_at)::text, 'never') FROM documents" 2>/dev/null || echo "never")
echo "\"latestSession\":\"$latest_session\",\"latestDocument\":\"$latest_doc\"}" >> "$OUTPUT"

echo "[export-portfolio-mcp] Written to: $OUTPUT" >&2
