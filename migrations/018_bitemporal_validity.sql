-- Bi-temporal validity for memories: valid_at is when the fact became true
-- in the world (may predate created_at, e.g. backfilled/imported data);
-- invalid_at is when it stopped being true, set only when a contradiction
-- is auto-resolved in its favor's opponent. Distinct from created_at/
-- updated_at, which track when *we* recorded the row, not when the fact held.
ALTER TABLE memories ADD COLUMN IF NOT EXISTS valid_at TIMESTAMPTZ;
ALTER TABLE memories ADD COLUMN IF NOT EXISTS invalid_at TIMESTAMPTZ;
