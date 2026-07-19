-- Shared event ledger for offline-capable Mac/VPS synchronization.  The same
-- schema is installed locally and on Neon; only local databases install
-- capture triggers through neon-sync migrate.
CREATE SCHEMA IF NOT EXISTS sync_meta;

CREATE TABLE IF NOT EXISTS sync_meta.events (
  event_id UUID PRIMARY KEY,
  device_id TEXT NOT NULL,
  logical_time BIGINT NOT NULL,
  table_name TEXT NOT NULL,
  record_key TEXT NOT NULL,
  operation TEXT NOT NULL CHECK (operation IN ('upsert','archive','delete')),
  payload JSONB NOT NULL DEFAULT '{}'::jsonb,
  payload_checksum TEXT NOT NULL,
  supersedes UUID,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  pushed_at TIMESTAMPTZ,
  UNIQUE(device_id, logical_time)
);
ALTER TABLE sync_meta.events ADD COLUMN IF NOT EXISTS pushed_at TIMESTAMPTZ;
CREATE INDEX IF NOT EXISTS idx_sync_events_created ON sync_meta.events(created_at, event_id);
CREATE INDEX IF NOT EXISTS idx_sync_events_entity ON sync_meta.events(table_name, record_key, created_at);

CREATE TABLE IF NOT EXISTS sync_meta.cursors (
  cursor_name TEXT PRIMARY KEY,
  cursor_value JSONB NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- A target-side lease provides a fencing token that works across Mac and VPS.
CREATE TABLE IF NOT EXISTS sync_meta.leases (
  lease_name TEXT PRIMARY KEY,
  device_id TEXT NOT NULL,
  generation BIGINT NOT NULL DEFAULT 1,
  expires_at TIMESTAMPTZ NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS sync_meta.conflicts (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  event_id UUID NOT NULL, table_name TEXT NOT NULL, record_key TEXT NOT NULL,
  reason TEXT NOT NULL, local_payload JSONB, remote_payload JSONB,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(), resolved_at TIMESTAMPTZ
);
