-- ============================================================
-- Migration: Hybrid Search + Memory Decay + Contradiction
-- Applies to existing deployments without dropping data.
-- ============================================================

-- 1. Add decay tracking columns to memories
DO $$ BEGIN
    ALTER TABLE memories ADD COLUMN IF NOT EXISTS last_accessed_at TIMESTAMPTZ;
EXCEPTION WHEN duplicate_column THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE memories ADD COLUMN IF NOT EXISTS access_count INT DEFAULT 0;
EXCEPTION WHEN duplicate_column THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE memories ADD COLUMN IF NOT EXISTS decay_score FLOAT DEFAULT 1.0;
EXCEPTION WHEN duplicate_column THEN NULL;
END $$;

-- 2. Add FTS tsvector column for BM25-style search
DO $$ BEGIN
    ALTER TABLE memories ADD COLUMN IF NOT EXISTS fts TSVECTOR;
EXCEPTION WHEN duplicate_column THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE documents ADD COLUMN IF NOT EXISTS fts TSVECTOR;
EXCEPTION WHEN duplicate_column THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE experiences ADD COLUMN IF NOT EXISTS fts TSVECTOR;
EXCEPTION WHEN duplicate_column THEN NULL;
END $$;

-- 3. Update FTS vectors with existing content (one-time backfill)
UPDATE memories SET fts = to_tsvector('english', COALESCE(content, '')) WHERE fts IS NULL;
UPDATE documents SET fts = to_tsvector('english', COALESCE(content, '')) WHERE fts IS NULL;
UPDATE experiences SET fts = to_tsvector('english', COALESCE(goal || ' ' || COALESCE(lessons_learned, ''), '')) WHERE fts IS NULL;

-- 4. Create GIN indexes on tsvector columns for fast FTS
CREATE INDEX IF NOT EXISTS idx_memories_fts ON memories USING gin(fts);
CREATE INDEX IF NOT EXISTS idx_documents_fts ON documents USING gin(fts);
CREATE INDEX IF NOT EXISTS idx_experiences_fts ON experiences USING gin(fts);

-- 5. Create triggers to auto-update FTS on content change
CREATE OR REPLACE FUNCTION update_memories_fts() RETURNS trigger AS $$
BEGIN
    NEW.fts := to_tsvector('english', COALESCE(NEW.content, ''));
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION update_documents_fts() RETURNS trigger AS $$
BEGIN
    NEW.fts := to_tsvector('english', COALESCE(NEW.content, ''));
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION update_experiences_fts() RETURNS trigger AS $$
BEGIN
    NEW.fts := to_tsvector('english', COALESCE(NEW.goal || ' ' || COALESCE(NEW.lessons_learned, ''), ''));
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Drop existing triggers first, then recreate
DROP TRIGGER IF EXISTS trg_memories_fts ON memories;
CREATE TRIGGER trg_memories_fts BEFORE INSERT OR UPDATE OF content ON memories
    FOR EACH ROW EXECUTE FUNCTION update_memories_fts();

DROP TRIGGER IF EXISTS trg_documents_fts ON documents;
CREATE TRIGGER trg_documents_fts BEFORE INSERT OR UPDATE OF content ON documents
    FOR EACH ROW EXECUTE FUNCTION update_documents_fts();

DROP TRIGGER IF EXISTS trg_experiences_fts ON experiences;
CREATE TRIGGER trg_experiences_fts BEFORE INSERT OR UPDATE OF goal, lessons_learned ON experiences
    FOR EACH ROW EXECUTE FUNCTION update_experiences_fts();

-- 6. Contradictions table
CREATE TABLE IF NOT EXISTS contradictions (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    memory_id_a     UUID NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    memory_id_b     UUID NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    content_a       TEXT NOT NULL,
    content_b       TEXT NOT NULL,
    similarity      FLOAT NOT NULL,          -- cosine similarity between embeddings
    contradiction_type TEXT DEFAULT 'semantic', -- 'semantic', 'temporal', 'factual'
    detected_by     TEXT DEFAULT 'auto',      -- 'auto' or 'human'
    resolved        BOOLEAN DEFAULT false,
    resolution_note TEXT,
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now(),
    CONSTRAINT chk_different_ids CHECK (memory_id_a <> memory_id_b)
);

CREATE INDEX IF NOT EXISTS idx_contradictions_mem_a ON contradictions(memory_id_a);
CREATE INDEX IF NOT EXISTS idx_contradictions_mem_b ON contradictions(memory_id_b);
CREATE INDEX IF NOT EXISTS idx_contradictions_unresolved ON contradictions(resolved) WHERE resolved = false;

-- 7. Decay config table (simple KV store for runtime settings)
CREATE TABLE IF NOT EXISTS config (
    key             TEXT PRIMARY KEY,
    value           TEXT NOT NULL,
    description     TEXT,
    updated_at      TIMESTAMPTZ DEFAULT now()
);

-- Insert default decay config if not exists
INSERT INTO config (key, value, description)
VALUES ('decay_half_life_days', '90', 'Number of days for memory score to decay by half (Ebbinghaus-inspired)')
ON CONFLICT (key) DO NOTHING;

INSERT INTO config (key, value, description)
VALUES ('decay_min_score', '0.1', 'Minimum decay score floor — memories never go below this')
ON CONFLICT (key) DO NOTHING;

INSERT INTO config (key, value, description)
VALUES ('decay_enabled', 'true', 'Enable/disable memory decay at query time')
ON CONFLICT (key) DO NOTHING;