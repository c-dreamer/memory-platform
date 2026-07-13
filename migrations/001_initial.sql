-- ============================================================
-- Memory Platform — Initialization
-- Run on first PostgreSQL startup (docker-entrypoint-initdb.d)
-- ============================================================

-- Vector similarity search (required for embedding queries)
CREATE EXTENSION IF NOT EXISTS vector;

-- Trigram full-text search (for keyword fallback)
CREATE EXTENSION IF NOT EXISTS pg_trgm;

-- UUID generation
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

-- ============================================================
-- Memory Platform — Full Database Schema
-- 15 tables with indexes for the memory system
-- ============================================================

-- ============================================================
-- AGENTS — registry of every entity that can write to memory
-- ============================================================
CREATE TABLE IF NOT EXISTS agents (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name            TEXT NOT NULL UNIQUE,
    agent_type      TEXT NOT NULL,          -- 'opencode', 'mt5', 'binance', 'btc', 'human', 'system'
    capabilities    TEXT[] DEFAULT '{}',
    metadata        JSONB DEFAULT '{}',
    is_active       BOOLEAN DEFAULT true,
    last_seen_at    TIMESTAMPTZ,
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now()
);

-- ============================================================
-- SESSIONS — task or conversation sessions
-- ============================================================
CREATE TABLE IF NOT EXISTS sessions (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id        UUID REFERENCES agents(id) ON DELETE SET NULL,
    parent_session_id UUID REFERENCES sessions(id) ON DELETE SET NULL,
    goal            TEXT,
    status          TEXT DEFAULT 'active',  -- 'active', 'completed', 'failed', 'abandoned'
    summary         TEXT,
    embedding       VECTOR(2048),
    started_at      TIMESTAMPTZ DEFAULT now(),
    ended_at        TIMESTAMPTZ,
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now()
);

-- ============================================================
-- MEMORIES — vector-indexed knowledge items (general purpose)
-- ============================================================
CREATE TABLE IF NOT EXISTS memories (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id        UUID REFERENCES agents(id) ON DELETE SET NULL,
    session_id      UUID REFERENCES sessions(id) ON DELETE SET NULL,
    content         TEXT NOT NULL,
    content_type    TEXT DEFAULT 'note',    -- 'note', 'observation', 'insight', 'decision', 'error'
    embedding       VECTOR(2048),
    fts             TSVECTOR,               -- for BM25 full-text search
    importance      FLOAT DEFAULT 0.5,     -- 0.0 (trivial) to 1.0 (critical)
    tags            TEXT[] DEFAULT '{}',
    metadata        JSONB DEFAULT '{}',
    last_accessed_at TIMESTAMPTZ,           -- for memory decay
    access_count    INT DEFAULT 0,           -- for memory decay
    decay_score     FLOAT DEFAULT 1.0,       -- computed decay multiplier
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now()
);

-- ============================================================
-- DOCUMENTS — full text from vault / ingested files (read-only mirror)
-- ============================================================
CREATE TABLE IF NOT EXISTS documents (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    path            TEXT NOT NULL UNIQUE,
    vault_section   TEXT,
    title           TEXT,
    content         TEXT NOT NULL,
    checksum        TEXT,
    frontmatter     JSONB DEFAULT '{}',
    embedding       VECTOR(2048),
    fts             TSVECTOR,
    token_count     INT,
    file_size_bytes INT,
    file_modified_at TIMESTAMPTZ,
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now()
);

-- ============================================================
-- PROJECTS — codebase / project registry
-- ============================================================
CREATE TABLE IF NOT EXISTS projects (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name            TEXT NOT NULL UNIQUE,
    description     TEXT,
    root_path       TEXT,
    repo_url        TEXT,
    language        TEXT,
    metadata        JSONB DEFAULT '{}',
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now()
);

-- ============================================================
-- CODE_CHANGES — coding task records
-- ============================================================
CREATE TABLE IF NOT EXISTS code_changes (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id        UUID REFERENCES agents(id) ON DELETE SET NULL,
    session_id      UUID REFERENCES sessions(id) ON DELETE SET NULL,
    project_id      UUID REFERENCES projects(id) ON DELETE SET NULL,
    problem         TEXT,
    solution        TEXT,
    files_changed   JSONB DEFAULT '[]',
    commit_hash     TEXT,
    branch          TEXT,
    embedding       VECTOR(2048),
    tags            TEXT[] DEFAULT '{}',
    created_at      TIMESTAMPTZ DEFAULT now()
);

-- ============================================================
-- TRADING_RESULTS — backtest / live trade records
-- ============================================================
CREATE TABLE IF NOT EXISTS trading_results (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id        UUID REFERENCES agents(id) ON DELETE SET NULL,
    ea_version      TEXT,
    strategy        TEXT,
    symbol          TEXT,
    timeframe       TEXT,
    trade_type      TEXT,                   -- 'backtest', 'live', 'forward_test'
    direction       TEXT,                   -- 'long', 'short'
    entry_price     DOUBLE PRECISION,
    exit_price      DOUBLE PRECISION,
    profit_factor   DOUBLE PRECISION,
    drawdown        DOUBLE PRECISION,
    win_rate        DOUBLE PRECISION,
    total_trades    INT,
    net_profit      DOUBLE PRECISION,
    duration_days   INT,
    indicators      JSONB DEFAULT '{}',
    inputs          JSONB DEFAULT '{}',
    notes           TEXT,
    embedding       VECTOR(2048),
    created_at      TIMESTAMPTZ DEFAULT now()
);

-- ============================================================
-- EXPERIENCES — completed task experiences for replay
-- ============================================================
CREATE TABLE IF NOT EXISTS experiences (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id        UUID REFERENCES agents(id) ON DELETE SET NULL,
    session_id      UUID REFERENCES sessions(id) ON DELETE SET NULL,
    goal            TEXT NOT NULL,
    reasoning_summary TEXT,
    actions         JSONB DEFAULT '[]',
    files_changed   JSONB DEFAULT '[]',
    result          TEXT,                   -- 'success', 'failure', 'partial'
    lessons_learned TEXT,
    confidence      FLOAT DEFAULT 0.0,
    duration_seconds INT,
    tags            TEXT[] DEFAULT '{}',
    related_project TEXT,
    embedding       VECTOR(2048),
    fts             TSVECTOR,
    is_procedurized BOOLEAN DEFAULT false,
    created_at      TIMESTAMPTZ DEFAULT now()
);

-- ============================================================
-- PROCEDURES — reusable workflows (generated, not hand-written)
-- ============================================================
CREATE TABLE IF NOT EXISTS procedures (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name            TEXT NOT NULL UNIQUE,
    description     TEXT,
    steps           JSONB NOT NULL,
    trigger_pattern TEXT,
    source_experience_id UUID REFERENCES experiences(id) ON DELETE SET NULL,
    tags            TEXT[] DEFAULT '{}',
    success_rate    FLOAT DEFAULT 0.0,
    times_used      INT DEFAULT 0,
    confidence      FLOAT DEFAULT 0.0,
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now()
);

-- ============================================================
-- RELATIONSHIPS — entity-relationship graph (mirrored in Graphiti)
-- ============================================================
CREATE TABLE IF NOT EXISTS relationships (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_type     TEXT NOT NULL,
    source_id       UUID NOT NULL,
    target_type     TEXT NOT NULL,
    target_id       UUID NOT NULL,
    relation_type   TEXT NOT NULL,
    weight          FLOAT DEFAULT 1.0,
    metadata        JSONB DEFAULT '{}',
    created_at      TIMESTAMPTZ DEFAULT now()
);

-- ============================================================
-- SUMMARIES — compressed context snapshots
-- ============================================================
CREATE TABLE IF NOT EXISTS summaries (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id      UUID REFERENCES sessions(id) ON DELETE CASCADE,
    source_type     TEXT,                   -- 'session', 'project', 'day'
    content         TEXT NOT NULL,
    embedding       VECTOR(2048),
    token_count     INT,
    created_at      TIMESTAMPTZ DEFAULT now()
);

-- ============================================================
-- EMBEDDINGS — universal embedding store (for cross-table search)
-- ============================================================
CREATE TABLE IF NOT EXISTS embeddings (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_table    TEXT NOT NULL,
    source_id       UUID NOT NULL,
    embedding       VECTOR(2048) NOT NULL,
    model           TEXT DEFAULT 'nvidia/llama-nemotron-embed-1b-v2',
    dimension       INT DEFAULT 2048,
    created_at      TIMESTAMPTZ DEFAULT now()
);

-- ============================================================
-- CONTRADICTIONS — detected conflicts between memories
-- ============================================================
CREATE TABLE IF NOT EXISTS contradictions (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    memory_id_a     UUID NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    memory_id_b     UUID NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    content_a       TEXT NOT NULL,
    content_b       TEXT NOT NULL,
    similarity      FLOAT NOT NULL,
    contradiction_type TEXT DEFAULT 'semantic',
    detected_by     TEXT DEFAULT 'auto',
    resolved        BOOLEAN DEFAULT false,
    resolution_note TEXT,
    created_at      TIMESTAMPTZ DEFAULT now(),
    updated_at      TIMESTAMPTZ DEFAULT now(),
    CONSTRAINT chk_different_ids CHECK (memory_id_a <> memory_id_b)
);

-- ============================================================
-- CONFIG — runtime configuration KV store
-- ============================================================
CREATE TABLE IF NOT EXISTS config (
    key             TEXT PRIMARY KEY,
    value           TEXT NOT NULL,
    description     TEXT,
    updated_at      TIMESTAMPTZ DEFAULT now()
);

-- ============================================================
-- INDEXES
-- ============================================================

-- Vector indexes — skipped because pgvector's IVFFlat/HNSW only support ≤2000 dims
-- and this deployment uses 2048-dim NVIDIA embeddings. Exact search with <=> suffices
-- at current data volumes (~1500 rows per table).

-- Full-text search indexes (BM25 via tsvector)
CREATE INDEX IF NOT EXISTS idx_memories_fts ON memories USING gin(fts);
CREATE INDEX IF NOT EXISTS idx_documents_fts ON documents USING gin(fts);
CREATE INDEX IF NOT EXISTS idx_experiences_fts ON experiences USING gin(fts);

-- Legacy trigram indexes (fallback)
CREATE INDEX IF NOT EXISTS idx_documents_content_trgm ON documents USING gin (content gin_trgm_ops);
CREATE INDEX IF NOT EXISTS idx_memories_content_trgm ON memories USING gin (content gin_trgm_ops);

-- B-tree indexes for common queries
CREATE INDEX IF NOT EXISTS idx_memories_agent ON memories(agent_id);
CREATE INDEX IF NOT EXISTS idx_memories_tags ON memories USING gin(tags);
CREATE INDEX IF NOT EXISTS idx_sessions_agent ON sessions(agent_id);
CREATE INDEX IF NOT EXISTS idx_sessions_status ON sessions(status);
CREATE INDEX IF NOT EXISTS idx_documents_section ON documents(vault_section);
CREATE INDEX IF NOT EXISTS idx_experiences_tags ON experiences USING gin(tags);
CREATE INDEX IF NOT EXISTS idx_experiences_result ON experiences(result);
CREATE INDEX IF NOT EXISTS idx_trading_results_symbol ON trading_results(symbol);
CREATE INDEX IF NOT EXISTS idx_trading_results_ea_version ON trading_results(ea_version);
CREATE INDEX IF NOT EXISTS idx_relationships_source ON relationships(source_type, source_id);
CREATE INDEX IF NOT EXISTS idx_relationships_target ON relationships(target_type, target_id);
CREATE INDEX IF NOT EXISTS idx_embeddings_source ON embeddings(source_table, source_id);
CREATE INDEX IF NOT EXISTS idx_procedures_trigger ON procedures(trigger_pattern);
CREATE INDEX IF NOT EXISTS idx_contradictions_unresolved ON contradictions(resolved) WHERE resolved = false;

-- ============================================================
-- FTS TRIGGERS — auto-update tsvector on content change
-- ============================================================
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

DROP TRIGGER IF EXISTS trg_memories_fts ON memories;
CREATE TRIGGER trg_memories_fts BEFORE INSERT OR UPDATE OF content ON memories
    FOR EACH ROW EXECUTE FUNCTION update_memories_fts();
DROP TRIGGER IF EXISTS trg_documents_fts ON documents;
CREATE TRIGGER trg_documents_fts BEFORE INSERT OR UPDATE OF content ON documents
    FOR EACH ROW EXECUTE FUNCTION update_documents_fts();
DROP TRIGGER IF EXISTS trg_experiences_fts ON experiences;
CREATE TRIGGER trg_experiences_fts BEFORE INSERT OR UPDATE OF goal, lessons_learned ON experiences
    FOR EACH ROW EXECUTE FUNCTION update_experiences_fts();
