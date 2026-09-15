-- store_contradiction's `ON CONFLICT DO NOTHING` had no unique constraint to
-- arbitrate against, so it was dead code: the same pair discovered from
-- either detect() direction inserted a fresh duplicate row every time
-- instead of deduplicating. store_contradiction now normalizes
-- (memory_id_a, memory_id_b) into canonical (smaller, larger) order before
-- insert, so a plain unique constraint on the raw column pair is
-- sufficient; no expression index needed.
--
-- Existing rows must be repaired before the index can be created, or this
-- migration hard-fails on every database that ran the old code (and a failed
-- migration is never recorded, so it would retry and fail forever, blocking
-- startup). Two separate repairs are needed, in this order:

-- 1. Normalize legacy rows into canonical id order. The contents travel with
--    their ids, or the swap would file one memory's text under the other's
--    id. Rows already canonical are untouched.
UPDATE contradictions
SET memory_id_a = memory_id_b,
    memory_id_b = memory_id_a,
    content_a   = content_b,
    content_b   = content_a
WHERE memory_id_a > memory_id_b;

-- 2. Collapse duplicate pairs, keeping the single most informative row: a
--    resolved row beats an unresolved one (it carries a human decision that
--    migration 016 made possible), then the most recently updated, then the
--    id as a final tiebreak so the ordering is total and exactly one row per
--    pair survives. COALESCE guards the nullable columns — a NULL in the
--    comparison would yield NULL, delete nothing, and leave the index
--    creation below to fail.
DELETE FROM contradictions c
USING contradictions keep
WHERE c.memory_id_a = keep.memory_id_a
  AND c.memory_id_b = keep.memory_id_b
  AND c.id <> keep.id
  AND (COALESCE(keep.resolved, false),
       COALESCE(keep.updated_at, keep.created_at, '-infinity'::timestamptz),
       keep.id)
    > (COALESCE(c.resolved, false),
       COALESCE(c.updated_at, c.created_at, '-infinity'::timestamptz),
       c.id);

-- A unique index (not a named CONSTRAINT — ALTER TABLE ... ADD CONSTRAINT
-- has no IF NOT EXISTS in Postgres) serves ON CONFLICT (memory_id_a,
-- memory_id_b) just as well as a table constraint would.
CREATE UNIQUE INDEX IF NOT EXISTS contradictions_pair_unique ON contradictions (memory_id_a, memory_id_b);
