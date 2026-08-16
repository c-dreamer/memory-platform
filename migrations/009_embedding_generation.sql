-- Add embedding_generation to the embeddings cache so the dimension guard can
-- distinguish vectors produced by a specific model/generation label even when
-- the vector dimension is identical (e.g. two different 2048-dim models).
--
-- Backfills existing rows with the canonical generation so historical vectors
-- remain addressable. Safe to apply to an already-correct schema.

ALTER TABLE embeddings
    ADD COLUMN IF NOT EXISTS embedding_generation TEXT DEFAULT 'nvidia-2048-v1';

UPDATE embeddings
    SET embedding_generation = 'nvidia-2048-v1'
    WHERE embedding_generation IS NULL;

CREATE INDEX IF NOT EXISTS idx_embeddings_generation
    ON embeddings(embedding_generation);