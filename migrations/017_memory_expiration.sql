-- Soft, date-bounded hide for memories — a third lifecycle primitive distinct
-- from decay (continuous relevance re-ranking) and delete (gone for good).
-- Once past, a memory is excluded from search/list but stays directly
-- fetchable (recall/get_memory) for point-in-time review, same spirit as
-- migration 016's superseded_by.
ALTER TABLE memories ADD COLUMN IF NOT EXISTS expiration_date DATE;

CREATE INDEX IF NOT EXISTS idx_memories_expiration_date ON memories(expiration_date) WHERE expiration_date IS NOT NULL;
