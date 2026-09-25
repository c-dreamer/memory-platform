-- expiration_date / CURRENT_DATE comparisons are evaluated per-Postgres-server
-- with no session TimeZone pinned anywhere in the codebase
-- (src/db/postgres.rs's .connect() call sites are bare, unlike
-- src/bin/neon-sync.rs which already pins other session GUCs on connect).
-- Not required for this migration, but worth pinning TimeZone the same way
-- if the fleet's Postgres instances ever run in different zones.

DROP INDEX IF EXISTS idx_memories_superseded_by;
DROP INDEX IF EXISTS idx_memories_expiration_date;

CREATE INDEX IF NOT EXISTS idx_memories_created_at ON memories (created_at DESC);

-- Repair before constraining. A failed migration is never recorded in
-- _migrations, so it retries forever and blocks startup — the same trap
-- migration 020 had to work around.
UPDATE memories SET importance = LEAST(GREATEST(importance, 0.0), 1.0)
WHERE importance < 0.0 OR importance > 1.0;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'memories_importance_bounds'
    ) THEN
        ALTER TABLE memories
            ADD CONSTRAINT memories_importance_bounds
            CHECK (importance >= 0.0 AND importance <= 1.0);
    END IF;
END $$;
