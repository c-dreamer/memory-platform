-- Rebuildable hierarchy for evidence-grounded retrieval. Source records remain
-- authoritative; deleting a node never deletes its source material.
CREATE TABLE IF NOT EXISTS content_nodes (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_table TEXT NOT NULL,
    source_id UUID NOT NULL,
    node_key TEXT NOT NULL,
    parent_key TEXT,
    root_scope TEXT NOT NULL,
    node_type TEXT NOT NULL,
    ordinal INTEGER NOT NULL DEFAULT 0,
    title TEXT NOT NULL,
    section_path TEXT NOT NULL,
    content TEXT NOT NULL DEFAULT '',
    content_start INTEGER,
    content_end INTEGER,
    checksum TEXT NOT NULL,
    source_checksum TEXT,
    sensitivity TEXT NOT NULL DEFAULT 'standard',
    storage_tier TEXT NOT NULL DEFAULT 'active'
        CHECK (storage_tier IN ('active','archive_pending','archived','restore_pending','superseded')),
    observed_at TIMESTAMPTZ,
    metadata JSONB NOT NULL DEFAULT '{}',
    fts TSVECTOR,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(source_table, source_id, node_key)
);

CREATE INDEX IF NOT EXISTS idx_content_nodes_scope_active
    ON content_nodes(root_scope, source_table, source_id)
    WHERE storage_tier = 'active';
CREATE INDEX IF NOT EXISTS idx_content_nodes_parent ON content_nodes(parent_key);
CREATE INDEX IF NOT EXISTS idx_content_nodes_fts ON content_nodes USING GIN(fts);

CREATE OR REPLACE FUNCTION content_nodes_fts_update() RETURNS trigger AS $$
BEGIN
    NEW.fts := to_tsvector('english', coalesce(NEW.title, '') || ' ' ||
                           coalesce(NEW.section_path, '') || ' ' ||
                           left(coalesce(NEW.content, ''), 200000));
    NEW.updated_at := now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_content_nodes_fts ON content_nodes;
CREATE TRIGGER trg_content_nodes_fts
BEFORE INSERT OR UPDATE OF title, section_path, content ON content_nodes
FOR EACH ROW EXECUTE FUNCTION content_nodes_fts_update();
