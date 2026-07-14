-- The initial schema used VECTOR(384) for code changes while the NVIDIA
-- backend uses 2048 dimensions everywhere else. Existing non-null values are
-- rejected rather than silently discarded.

DO $$
DECLARE
    vector_typmod INT;
BEGIN
    SELECT a.atttypmod
    INTO vector_typmod
    FROM pg_attribute a
    JOIN pg_class c ON c.oid = a.attrelid
    JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname = 'public'
      AND c.relname = 'code_changes'
      AND a.attname = 'embedding'
      AND a.attnum > 0;

    IF vector_typmod IS DISTINCT FROM 2052 THEN
        IF EXISTS (SELECT 1 FROM code_changes WHERE embedding IS NOT NULL) THEN
            RAISE EXCEPTION 'code_changes contains non-null embeddings with an unsupported dimension';
        END IF;

        ALTER TABLE code_changes
            ALTER COLUMN embedding TYPE VECTOR(2048)
            USING NULL;
    END IF;
END;
$$;
