-- Neon's own commit-order sequence for sync_meta.events, replacing the old
-- (created_at,event_id) wall-clock ordering in pull_events
-- (src/bin/neon-sync.rs). logical_time is assigned via an AFTER trigger and
-- is not itself transactional, so under this app's connection pool a
-- fast-committing higher-logical_time write can commit before a
-- slower-committing lower one -- reproducing the exact permanent-skip bug
-- this column exists to close, just on a different column. neon_seq is
-- instead assigned by Postgres's own nextval() at INSERT time inside
-- push_event_batch's single lease-serialized transaction, so on Neon it is
-- genuinely commit order. Applied to both local and target databases per
-- AGENTS.md; the value only has meaning as assigned by Neon's own sequence,
-- since push_event_batch's INSERT never lists it explicitly.
ALTER TABLE sync_meta.events ADD COLUMN IF NOT EXISTS neon_seq BIGSERIAL;
CREATE INDEX IF NOT EXISTS idx_sync_events_neon_seq ON sync_meta.events(neon_seq);
