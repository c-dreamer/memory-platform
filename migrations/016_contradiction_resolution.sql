-- Closes the second half of contradiction detection: until now `contradictions`
-- could be detected and stored but never resolved (no caller ever set
-- `resolved`/`resolution_note`). `superseded_by`/`superseded_at` let a
-- resolution pick a winner between two contradicting memories — "what's true
-- now" and "what we believed then" become separately recoverable instead of
-- the older belief just quietly losing search rank via decay_score alone.
ALTER TABLE memories ADD COLUMN IF NOT EXISTS superseded_by UUID REFERENCES memories(id) ON DELETE SET NULL;
ALTER TABLE memories ADD COLUMN IF NOT EXISTS superseded_at TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS idx_memories_superseded_by ON memories(superseded_by) WHERE superseded_by IS NOT NULL;
