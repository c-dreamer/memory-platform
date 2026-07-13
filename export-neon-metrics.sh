#!/usr/bin/env bash
set -euo pipefail

# Export Neon database metrics for Portfolio Radar's Neon metrics adapter.
# Writes a JSON export of table-level metrics.
# Usage: export-neon-metrics.sh [output-path]
# Defaults to $NEON_METRICS_EXPORT or /tmp/neon-metrics-export.json

OUTPUT="${1:-${NEON_METRICS_EXPORT:-/tmp/neon-metrics-export.json}}"

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
echo -n '"tables":{' >> "$OUTPUT"

FIRST=true
for table in documents memories experiences sessions agents procedures contradictions embeddings; do
  count=$($PSQL -c "SELECT count(*) FROM $table" 2>/dev/null || echo "0")
  $FIRST || echo -n ',' >> "$OUTPUT"
  FIRST=false
  echo -n "\"$table\":$count" >> "$OUTPUT"
done

db_size=$($PSQL -c "SELECT pg_size_pretty(pg_database_size('memory'))" 2>/dev/null || echo "unknown")
echo "},\"dbSize\":\"$db_size\"" >> "$OUTPUT"
echo "}" >> "$OUTPUT"

echo "[export-neon-metrics] Written to: $OUTPUT" >&2
