-- Declares which project (work|personal) a Neon database is designated for.
-- A neon-sync device whose MEMORY_SCOPE disagrees refuses to run (checked in
-- src/bin/neon-sync.rs main(), against the Neon side of this table), closing
-- the "personal Neon project holding client data" boundary risk with an
-- explicit marker rather than a naming convention alone. Applied to both
-- local and target databases per AGENTS.md; only the target's row is
-- consulted. Set via `neon-sync set-scope work|personal`, a one-time init.
CREATE TABLE IF NOT EXISTS sync_meta.scope (
    singleton BOOLEAN PRIMARY KEY DEFAULT true CHECK (singleton),
    scope TEXT NOT NULL CHECK (scope IN ('work', 'personal')),
    set_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
