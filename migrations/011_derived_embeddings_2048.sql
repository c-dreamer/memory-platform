-- Enforce the production dimension on the derived cache. The cache is
-- rebuildable from the operational embedding columns, so an incorrectly
-- typed cache is safely recreated rather than silently accepting 1024 vectors.
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
       AND c.relname = 'embeddings'
       AND a.attname = 'embedding'
       AND a.attnum > 0;

    IF vector_typmod IS NULL OR vector_typmod <> 2052 THEN
        DROP TABLE IF EXISTS public.embeddings;
        CREATE TABLE public.embeddings (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            source_table TEXT NOT NULL,
            source_id UUID NOT NULL,
            embedding VECTOR(2048) NOT NULL,
            model TEXT DEFAULT 'nvidia/llama-nemotron-embed-1b-v2',
            dimension INT DEFAULT 2048,
            created_at TIMESTAMPTZ DEFAULT now()
        );
    END IF;
END;
$$;

CREATE UNIQUE INDEX IF NOT EXISTS idx_embeddings_source
    ON public.embeddings(source_table, source_id);
