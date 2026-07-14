-- Storage tiers make Neon an active projection while local PostgreSQL keeps
-- the authoritative archive ledger and restoration metadata.
DO $$
DECLARE table_name TEXT;
BEGIN
  FOREACH table_name IN ARRAY ARRAY['agents','config','projects','sessions','documents','memories','experiences','procedures','summaries','code_changes','trading_results','contradictions','relationships','session_documents','session_memories']
  LOOP
    EXECUTE format('ALTER TABLE public.%I ADD COLUMN IF NOT EXISTS storage_tier TEXT NOT NULL DEFAULT ''active'' CHECK (storage_tier IN (''active'',''archive_pending'',''archived'',''restore_pending'',''superseded''))', table_name);
    EXECUTE format('CREATE INDEX IF NOT EXISTS idx_%I_storage_tier ON public.%I(storage_tier)', table_name, table_name);
  END LOOP;
END $$;

ALTER TABLE documents ADD COLUMN IF NOT EXISTS source_checksum TEXT;
ALTER TABLE documents ADD COLUMN IF NOT EXISTS archive_id UUID;
ALTER TABLE memories ADD COLUMN IF NOT EXISTS embedding_model TEXT DEFAULT 'nvidia/llama-nemotron-embed-1b-v2';
ALTER TABLE memories ADD COLUMN IF NOT EXISTS embedding_dimension INT DEFAULT 2048;
ALTER TABLE memories ADD COLUMN IF NOT EXISTS embedding_generation TEXT DEFAULT 'nvidia-2048-v1';
ALTER TABLE memories ADD COLUMN IF NOT EXISTS embedded_at TIMESTAMPTZ;
ALTER TABLE memories ADD COLUMN IF NOT EXISTS archive_id UUID;
ALTER TABLE memories ADD COLUMN IF NOT EXISTS observed_at TIMESTAMPTZ;
ALTER TABLE memories ADD COLUMN IF NOT EXISTS valid_until TIMESTAMPTZ;

CREATE SCHEMA IF NOT EXISTS archive_meta;
CREATE TABLE IF NOT EXISTS archive_meta.bundles (
  archive_id UUID PRIMARY KEY DEFAULT gen_random_uuid(), local_path TEXT NOT NULL,
  remote_path TEXT NOT NULL UNIQUE, manifest_checksum TEXT NOT NULL, byte_count BIGINT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('building','uploaded','verified','failed')),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), verified_at TIMESTAMPTZ
);
CREATE TABLE IF NOT EXISTS archive_meta.records (
  archive_id UUID NOT NULL REFERENCES archive_meta.bundles(archive_id), table_name TEXT NOT NULL,
  record_key TEXT NOT NULL, source_checksum TEXT, reason TEXT NOT NULL, device_id TEXT NOT NULL,
  state TEXT NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT now(), restored_at TIMESTAMPTZ,
  PRIMARY KEY (table_name, record_key, archive_id)
);
CREATE TABLE IF NOT EXISTS archive_meta.leases (
  table_name TEXT NOT NULL, record_key TEXT NOT NULL, device_id TEXT NOT NULL,
  generation BIGINT NOT NULL DEFAULT 1, expires_at TIMESTAMPTZ NOT NULL,
  PRIMARY KEY (table_name, record_key)
);
