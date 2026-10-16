-- Source tier transitions must take effect even if the index worker is offline.
-- Raw sources are never altered by these derived-cache maintenance triggers.
CREATE OR REPLACE FUNCTION invalidate_content_nodes() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        DELETE FROM content_nodes WHERE source_table=TG_TABLE_NAME AND source_id=OLD.id;
        RETURN OLD;
    END IF;
    IF NEW.storage_tier IS DISTINCT FROM OLD.storage_tier THEN
        UPDATE content_nodes SET storage_tier=NEW.storage_tier, updated_at=now()
        WHERE source_table=TG_TABLE_NAME AND source_id=NEW.id
          AND storage_tier IS DISTINCT FROM NEW.storage_tier;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DO $$
DECLARE source TEXT;
BEGIN
    FOREACH source IN ARRAY ARRAY['documents','memories','sessions'] LOOP
        EXECUTE format('DROP TRIGGER IF EXISTS trg_hierarchy_source_lifecycle ON %I',source);
        EXECUTE format('CREATE TRIGGER trg_hierarchy_source_lifecycle AFTER UPDATE OF storage_tier OR DELETE ON %I FOR EACH ROW EXECUTE FUNCTION invalidate_content_nodes()',source);
    END LOOP;
END $$;
