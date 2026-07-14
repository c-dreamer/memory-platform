-- Rebuild the derived embeddings cache only when it is not already 2048-dim.
-- This migration is intentionally safe to apply to an already-correct cache.

DO $$
DECLARE
    vector_typmod INT;
BEGIN
    SELECT a.atttypmod
    INTO vector_typmod
    FROM pg_attribute a
    JOIN pg_class c ON c.oid = a.attrelid
    JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname = 'public'
      AND c.relname = 'embeddings'
      AND a.attname = 'embedding'
      AND a.attnum > 0;

    IF vector_typmod IS DISTINCT FROM 2052 THEN
        DROP TABLE IF EXISTS embeddings;
        CREATE TABLE embeddings (
            id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            source_table    TEXT NOT NULL,
            source_id       UUID NOT NULL,
            embedding       VECTOR(2048) NOT NULL,
            model           TEXT DEFAULT 'nvidia/llama-nemotron-embed-1b-v2',
            dimension       INT DEFAULT 2048,
            created_at      TIMESTAMPTZ DEFAULT now()
        );
    END IF;
END;
$$;

CREATE UNIQUE INDEX IF NOT EXISTS idx_embeddings_source
    ON embeddings(source_table, source_id);
