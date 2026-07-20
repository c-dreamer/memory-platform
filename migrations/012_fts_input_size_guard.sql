-- PostgreSQL tsvector has a 1 MiB limit. Preserve full source content while
-- limiting only the derived FTS input for very large logs/documents.
CREATE OR REPLACE FUNCTION update_memories_fts() RETURNS trigger AS $$
BEGIN
    NEW.fts := to_tsvector('english', left(COALESCE(NEW.content, ''), 500000));
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION update_documents_fts() RETURNS trigger AS $$
BEGIN
    NEW.fts := to_tsvector('english', left(COALESCE(NEW.content, ''), 500000));
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION update_experiences_fts() RETURNS trigger AS $$
BEGIN
    NEW.fts := to_tsvector('english', left(COALESCE(NEW.goal || ' ' || COALESCE(NEW.lessons_learned, ''), ''), 500000));
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
