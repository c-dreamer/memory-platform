-- Normalize embeddings source rows before enforcing uniqueness.
-- Keep the newest row for each (source_table, source_id) pair.

DELETE FROM embeddings a
USING embeddings b
WHERE a.source_table = b.source_table
  AND a.source_id = b.source_id
  AND (
    a.created_at < b.created_at
    OR (a.created_at = b.created_at AND a.id < b.id)
  );

DROP INDEX IF EXISTS idx_embeddings_source;

CREATE UNIQUE INDEX IF NOT EXISTS idx_embeddings_source
    ON embeddings(source_table, source_id);
