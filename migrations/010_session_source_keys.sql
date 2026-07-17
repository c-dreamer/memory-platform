-- Stable source identities make repeated OpenCode/Codex ingestion idempotent.
ALTER TABLE public.sessions ADD COLUMN IF NOT EXISTS source_key TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS idx_sessions_source_key
    ON public.sessions(source_key) WHERE source_key IS NOT NULL;
