-- Ensure mutable rows always advance their incremental-sync watermark.

ALTER TABLE experiences
    ADD COLUMN IF NOT EXISTS updated_at TIMESTAMPTZ DEFAULT now();

CREATE OR REPLACE FUNCTION maintain_updated_at() RETURNS trigger AS $$
BEGIN
    -- The synchronizer replays the authoritative source timestamp. Without
    -- this guard, a target-side maintenance trigger would manufacture drift.
    IF current_setting('memory.sync_replay', true) = 'on' THEN
        RETURN NEW;
    END IF;

    -- Preserve an explicit source timestamp during replication.
    IF NEW.updated_at IS NOT DISTINCT FROM OLD.updated_at THEN
        NEW.updated_at := now();
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DO $$
DECLARE
    table_name TEXT;
BEGIN
    FOREACH table_name IN ARRAY ARRAY[
        'agents', 'sessions', 'memories', 'documents', 'projects',
        'experiences', 'procedures', 'contradictions', 'config'
    ]
    LOOP
        EXECUTE format('DROP TRIGGER IF EXISTS trg_%I_updated_at ON %I', table_name, table_name);
        EXECUTE format(
            'CREATE TRIGGER trg_%I_updated_at BEFORE UPDATE ON %I '
            'FOR EACH ROW EXECUTE FUNCTION maintain_updated_at()',
            table_name,
            table_name
        );
    END LOOP;
END;
$$;
