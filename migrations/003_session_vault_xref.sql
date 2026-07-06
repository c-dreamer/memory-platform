-- ============================================================
-- Migration: Session↔Vault Cross-Reference Table
-- Tracks which documents were accessed in which session
-- with interaction type and relevance score.
-- ============================================================

-- 1. Session-documents cross-reference table
CREATE TABLE IF NOT EXISTS session_documents (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id      UUID NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    document_id     UUID NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    interaction_type TEXT NOT NULL,           -- 'loaded', 'stored', 'searched'
    relevance_score FLOAT8,
    metadata        JSONB DEFAULT '{}',
    accessed_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at      TIMESTAMPTZ DEFAULT now()
);

-- Indexes for fast lookups by session and document
CREATE INDEX IF NOT EXISTS idx_session_docs_session ON session_documents(session_id);
CREATE INDEX IF NOT EXISTS idx_session_docs_document ON session_documents(document_id);
CREATE INDEX IF NOT EXISTS idx_session_docs_interaction ON session_documents(interaction_type);
CREATE INDEX IF NOT EXISTS idx_session_docs_accessed ON session_documents(accessed_at);

-- 2. Session-memories cross-reference table (memories created/accessed in sessions)
CREATE TABLE IF NOT EXISTS session_memories (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id      UUID NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    memory_id       UUID NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    interaction_type TEXT NOT NULL,           -- 'created', 'loaded', 'searched', 'updated'
    relevance_score FLOAT8,
    metadata        JSONB DEFAULT '{}',
    accessed_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at      TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_session_mems_session ON session_memories(session_id);
CREATE INDEX IF NOT EXISTS idx_session_mems_memory ON session_memories(memory_id);
CREATE INDEX IF NOT EXISTS idx_session_mems_interaction ON session_memories(interaction_type);

-- 3. Function to record a document access within a session
CREATE OR REPLACE FUNCTION record_document_access(
    p_session_id UUID,
    p_document_id UUID,
    p_interaction_type TEXT,
    p_relevance_score FLOAT8 DEFAULT NULL
) RETURNS UUID AS $$
DECLARE
    xref_id UUID;
BEGIN
    INSERT INTO session_documents (session_id, document_id, interaction_type, relevance_score)
    VALUES (p_session_id, p_document_id, p_interaction_type, p_relevance_score)
    RETURNING id INTO xref_id;
    RETURN xref_id;
END;
$$ LANGUAGE plpgsql;

-- 4. Function to record a memory access within a session
CREATE OR REPLACE FUNCTION record_memory_access(
    p_session_id UUID,
    p_memory_id UUID,
    p_interaction_type TEXT,
    p_relevance_score FLOAT8 DEFAULT NULL
) RETURNS UUID AS $$
DECLARE
    xref_id UUID;
BEGIN
    INSERT INTO session_memories (session_id, memory_id, interaction_type, relevance_score)
    VALUES (p_session_id, p_memory_id, p_interaction_type, p_relevance_score)
    RETURNING id INTO xref_id;
    RETURN xref_id;
END;
$$ LANGUAGE plpgsql;

-- 5. Function to get all context for a session (documents + memories)
CREATE OR REPLACE FUNCTION get_session_context(p_session_id UUID)
RETURNS TABLE(
    entity_type TEXT,
    entity_id UUID,
    interaction_type TEXT,
    relevance_score FLOAT8,
    content TEXT,
    accessed_at TIMESTAMPTZ
) AS $$
BEGIN
    RETURN QUERY
    -- Documents accessed in session
    SELECT
        'document'::TEXT AS entity_type,
        sd.document_id AS entity_id,
        sd.interaction_type,
        sd.relevance_score,
        d.content,
        sd.accessed_at
    FROM session_documents sd
    JOIN documents d ON d.id = sd.document_id
    WHERE sd.session_id = p_session_id

    UNION ALL

    -- Memories accessed in session
    SELECT
        'memory'::TEXT AS entity_type,
        sm.memory_id AS entity_id,
        sm.interaction_type,
        sm.relevance_score,
        m.content,
        sm.accessed_at
    FROM session_memories sm
    JOIN memories m ON m.id = sm.memory_id
    WHERE sm.session_id = p_session_id

    ORDER BY accessed_at DESC;
END;
$$ LANGUAGE plpgsql;
