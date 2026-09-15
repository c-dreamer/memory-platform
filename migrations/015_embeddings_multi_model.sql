-- Widens public.embeddings (the derived cache table) to hold vectors from
-- more than one embedding model/dimensionality side by side, so the
-- llama.cpp-backed local backend (Qwen3-Embedding-4B, truncated+renormalized
-- to 2048 dims in Rust) can coexist with the existing NVIDIA-backed rows
-- instead of colliding on (source_table, source_id). Source tables
-- (memories, documents, experiences, ...) keep their own fixed VECTOR(2048)
-- columns unchanged -- only this cache needs cross-model rows.
--
-- `model` and `dimension` columns already exist (migration 005) but `model`
-- was never NOT NULL and store_embedding() never bound it explicitly,
-- relying on the column default -- so every row today already carries the
-- correct 'nvidia/llama-nemotron-embed-1b-v2' value via that default and can
-- be backfilled from it directly.
UPDATE embeddings SET model = 'nvidia/llama-nemotron-embed-1b-v2' WHERE model IS NULL;
ALTER TABLE embeddings ALTER COLUMN model SET NOT NULL;

ALTER TABLE embeddings ALTER COLUMN embedding DROP NOT NULL;
ALTER TABLE embeddings ALTER COLUMN embedding TYPE vector;

ALTER TABLE embeddings DROP CONSTRAINT IF EXISTS embeddings_dimension_matches_vector;
ALTER TABLE embeddings ADD CONSTRAINT embeddings_dimension_matches_vector
    CHECK (embedding IS NULL OR vector_dims(embedding) = dimension);

DROP INDEX IF EXISTS idx_embeddings_source;
CREATE UNIQUE INDEX IF NOT EXISTS idx_embeddings_source_model
    ON embeddings(source_table, source_id, model);
