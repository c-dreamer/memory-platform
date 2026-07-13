-- Rebuild the derived embeddings cache for the NVIDIA 2048-dim backend.
-- This table is a cache derived from source rows, so it is safe to recreate.

DROP TABLE IF EXISTS embeddings;

CREATE TABLE IF NOT EXISTS embeddings (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_table    TEXT NOT NULL,
    source_id       UUID NOT NULL,
    embedding       VECTOR(2048) NOT NULL,
    model           TEXT DEFAULT 'nvidia/llama-nemotron-embed-1b-v2',
    dimension       INT DEFAULT 2048,
    created_at      TIMESTAMPTZ DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_embeddings_source
    ON embeddings(source_table, source_id);
