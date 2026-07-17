-- The Neon mirror must expose the source primary key as a conflict target.
-- Some pre-rollout databases have the id column without a constraint; these
-- indexes are idempotent and safe because the synchronizer already requires
-- UUID uniqueness.
DO $$
DECLARE table_name TEXT;
BEGIN
  FOREACH table_name IN ARRAY ARRAY[
    'agents','projects','sessions','documents','memories',
    'experiences','procedures','summaries','code_changes','trading_results',
    'contradictions','relationships','session_documents','session_memories'
  ]
  LOOP
    IF to_regclass(format('public.%I', table_name)) IS NOT NULL THEN
      EXECUTE format('CREATE UNIQUE INDEX IF NOT EXISTS idx_%I_sync_id ON public.%I(id)', table_name, table_name);
    END IF;
  END LOOP;
END $$;
