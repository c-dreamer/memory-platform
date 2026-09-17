-- Migrations 016/017 indexed superseded_by/expiration_date the wrong way:
-- `WHERE ... IS NOT NULL` never matches the hot-path predicate, which is
-- always `IS NULL` (vector_search/bm25_search's superseded_filter) or
-- `IS NULL OR >= CURRENT_DATE` (expiration_filter) — the index can only
-- serve queries for exactly the rows every read excludes. Flip direction,
-- matching idx_contradictions_unresolved's established `resolved = false`
-- convention (migration 002: index the flag column, predicate = the
-- hot-path condition).
--
-- expiration_date's hot-path predicate is `IS NULL OR >= CURRENT_DATE`, but
-- CURRENT_DATE is STABLE not IMMUTABLE and can't appear in an index
-- predicate. IS NULL still covers the common never-expires case, which
-- every query short-circuits on first via OR.
DROP INDEX IF EXISTS idx_memories_superseded_by;
CREATE INDEX IF NOT EXISTS idx_memories_superseded_by ON memories(superseded_by) WHERE superseded_by IS NULL;

DROP INDEX IF EXISTS idx_memories_expiration_date;
CREATE INDEX IF NOT EXISTS idx_memories_expiration_date ON memories(expiration_date) WHERE expiration_date IS NULL;
