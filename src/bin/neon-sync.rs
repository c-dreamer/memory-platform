//! Resumable, low-bandwidth mirror from local PostgreSQL to Neon.
//!
//! The local database owns an outbox. Target writes are idempotent and an
//! outbox item is acknowledged only after its Neon transaction commits.

use std::{
    collections::{BTreeMap, HashMap},
    env,
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use serde_json::Value;
use sqlx::{
    encode::{Encode, IsNull},
    postgres::{PgConnectOptions, PgPoolOptions, PgTypeInfo},
    types::Type,
    PgPool, Postgres, Row, TypeInfo,
};
use uuid::Uuid;

use memory_platform::migrations::Migrator;

const DEFAULT_BATCH_ROWS: usize = 25;
const MIN_BATCH_ROWS: usize = 1;
const MAX_BATCH_BYTES: usize = 2 * 1024 * 1024;
const RUN_BUDGET: Duration = Duration::from_secs(10 * 60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const QUERY_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_RETRIES: usize = 4;

#[derive(Parser)]
#[command(
    name = "neon-sync",
    about = "Resumable local PostgreSQL to Neon mirror"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// Local authoritative PostgreSQL URL. Reads LOCAL_URL then DATABASE_URL.
    #[arg(long, env = "LOCAL_URL")]
    local_url: Option<String>,
    /// Neon direct PostgreSQL URL. Reads NEON_SYNC_URL, then NEON_DIRECT.
    #[arg(long, env = "NEON_SYNC_URL")]
    neon_url: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Compatibility alias for `push`.
    Run,
    /// Compatibility alias for the bounded audit command.
    Reconcile,
    /// Read-only queue, migration, lease, and lightweight count status.
    Status,
    /// Read-only endpoint and schema health check. Never runs DDL.
    Health,
    /// Apply ordered migrations and install local capture triggers.
    Migrate,
    /// Push durable local changes and events within the run budget.
    Push,
    /// Pull unseen remote events into the local durable inbox.
    Pull,
    /// Compare one resumable inventory page and enqueue only local repairs.
    Audit,
    /// Create bounded baseline events for an existing local corpus.
    Bootstrap,
    /// Manually enqueue and drain the complete active corpus. Never runs automatically.
    Full {
        /// Required acknowledgement for a potentially long, network-intensive recovery.
        #[arg(long)]
        confirm_full_push: bool,
    },
    /// Rebuild Neon full-text search and derived embedding cache without NVIDIA calls.
    RebuildDerived,
    /// Emergency-only destructive target reset. Never runs automatically.
    ResetTarget {
        #[arg(long)]
        confirm_neon_reset: bool,
    },
}

#[derive(Clone, Copy)]
struct TableSpec {
    name: &'static str,
    key: &'static str,
    natural_key: Option<&'static str>,
    has_embedding: bool,
}

// Parent-before-child order. `embeddings` is intentionally omitted: it is derived.
const TABLES: &[TableSpec] = &[
    TableSpec {
        name: "agents",
        key: "id",
        natural_key: Some("name"),
        has_embedding: false,
    },
    TableSpec {
        name: "config",
        key: "key",
        natural_key: None,
        has_embedding: false,
    },
    TableSpec {
        name: "projects",
        key: "id",
        natural_key: Some("name"),
        has_embedding: false,
    },
    TableSpec {
        name: "sessions",
        key: "id",
        natural_key: None,
        has_embedding: true,
    },
    TableSpec {
        name: "documents",
        key: "id",
        natural_key: Some("path"),
        has_embedding: true,
    },
    TableSpec {
        name: "memories",
        key: "id",
        natural_key: None,
        has_embedding: true,
    },
    TableSpec {
        name: "experiences",
        key: "id",
        natural_key: None,
        has_embedding: true,
    },
    TableSpec {
        name: "procedures",
        key: "id",
        natural_key: Some("name"),
        has_embedding: false,
    },
    TableSpec {
        name: "summaries",
        key: "id",
        natural_key: None,
        has_embedding: true,
    },
    TableSpec {
        name: "code_changes",
        key: "id",
        natural_key: None,
        has_embedding: true,
    },
    TableSpec {
        name: "trading_results",
        key: "id",
        natural_key: None,
        has_embedding: true,
    },
    TableSpec {
        name: "contradictions",
        key: "id",
        natural_key: None,
        has_embedding: false,
    },
    TableSpec {
        name: "relationships",
        key: "id",
        natural_key: None,
        has_embedding: false,
    },
    TableSpec {
        name: "session_documents",
        key: "id",
        natural_key: None,
        has_embedding: false,
    },
    TableSpec {
        name: "session_memories",
        key: "id",
        natural_key: None,
        has_embedding: false,
    },
];

#[derive(Debug)]
struct OutboxItem {
    table: String,
    key: String,
    generation: i64,
    op: String,
}

#[derive(Debug, Clone)]
struct SourceRow {
    key: String,
    data: Value,
    vector: Option<PgVector>,
}

/// pgvector binary wire format: int16 dimensions, int16 unused, big-endian f32 values.
#[derive(Debug, Clone)]
struct PgVector(Vec<f32>);

impl Type<Postgres> for PgVector {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::with_name("vector")
    }
    fn compatible(ty: &PgTypeInfo) -> bool {
        ty.name() == "vector"
    }
}

impl<'q> Encode<'q, Postgres> for PgVector {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> std::result::Result<IsNull, Box<dyn std::error::Error + Send + Sync>> {
        let dimensions: i16 = self
            .0
            .len()
            .try_into()
            .map_err(|_| "vector has too many dimensions")?;
        buf.extend_from_slice(&dimensions.to_be_bytes());
        buf.extend_from_slice(&0_i16.to_be_bytes());
        for value in &self.0 {
            buf.extend_from_slice(&value.to_be_bytes());
        }
        Ok(IsNull::No)
    }
    fn size_hint(&self) -> usize {
        4 + self.0.len() * 4
    }
}

fn quote_ident(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn vector_from_text(input: &str) -> Result<PgVector> {
    let trimmed = input.trim().trim_start_matches('[').trim_end_matches(']');
    if trimmed.is_empty() {
        return Ok(PgVector(Vec::new()));
    }
    trimmed
        .split(',')
        .map(|part| {
            part.trim()
                .parse::<f32>()
                .context("invalid pgvector component")
        })
        .collect::<Result<Vec<_>>>()
        .map(PgVector)
}

fn urls(cli: &Cli) -> Result<(String, String)> {
    let local = cli
        .local_url
        .clone()
        .or_else(|| env::var("DATABASE_URL").ok())
        .ok_or_else(|| anyhow!("LOCAL_URL or DATABASE_URL is required"))?;
    let neon = cli
        .neon_url
        .clone()
        .or_else(|| env::var("NEON_DIRECT").ok())
        .or_else(|| env::var("NEON_SYNC_URL").ok())
        .ok_or_else(|| anyhow!("NEON_SYNC_URL or NEON_DIRECT is required"))?;
    if neon.contains("-pooler") {
        bail!("NEON_SYNC_URL must be a direct endpoint; refusing to rewrite a pooler URL")
    }
    Ok((local, neon))
}

async fn connect(url: &str, label: &str) -> Result<PgPool> {
    let options = url
        .parse::<PgConnectOptions>()
        .context("invalid PostgreSQL URL")?;
    let mut last_error = None;
    for attempt in 0..=MAX_RETRIES {
        let connection = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(CONNECT_TIMEOUT)
            .connect_with(options.clone());
        match tokio::time::timeout(CONNECT_TIMEOUT, connection).await {
            Ok(Ok(pool)) => return Ok(pool),
            Ok(Err(error)) => last_error = Some(error.to_string()),
            Err(_) => last_error = Some("connection attempt timed out".to_string()),
        }
        if attempt < MAX_RETRIES {
            tokio::time::sleep(Duration::from_millis(250 * (1 << attempt))).await;
        }
    }
    bail!(
        "could not connect to {label} after {} bounded attempts: {}",
        MAX_RETRIES + 1,
        last_error.unwrap_or_else(|| "unknown connection error".to_string())
    )
}

async fn ensure_local_queue(local: &PgPool) -> Result<()> {
    sqlx::raw_sql(r#"
CREATE SCHEMA IF NOT EXISTS sync_meta;
CREATE TABLE IF NOT EXISTS sync_meta.outbox (
    table_name TEXT NOT NULL,
    record_key TEXT NOT NULL,
    generation BIGINT NOT NULL DEFAULT 1,
    operation TEXT NOT NULL CHECK (operation IN ('upsert', 'delete')),
    changed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    attempts INT NOT NULL DEFAULT 0,
    last_error TEXT,
    PRIMARY KEY (table_name, record_key)
);
CREATE TABLE IF NOT EXISTS sync_meta.state (
    state_key TEXT PRIMARY KEY,
    state_value JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS sync_meta.runtime (
    singleton BOOLEAN PRIMARY KEY DEFAULT true CHECK (singleton),
    device_id TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE SEQUENCE IF NOT EXISTS sync_meta.logical_clock;
CREATE OR REPLACE FUNCTION sync_meta.capture_change() RETURNS trigger AS $$
DECLARE
    record_value JSONB;
    event_payload JSONB;
    row_key TEXT;
    device TEXT;
BEGIN
    IF current_setting('memory.sync_replay', true) = 'on' THEN
        RETURN COALESCE(NEW, OLD);
    END IF;
    IF TG_OP = 'UPDATE' AND (to_jsonb(NEW) - 'updated_at') IS NOT DISTINCT FROM (to_jsonb(OLD) - 'updated_at') THEN
        RETURN NEW;
    END IF;
    record_value := CASE WHEN TG_OP = 'DELETE' THEN to_jsonb(OLD) ELSE to_jsonb(NEW) END;
    row_key := record_value ->> TG_ARGV[0];
    SELECT device_id INTO device FROM sync_meta.runtime WHERE singleton;
    IF device IS NULL THEN
        RAISE EXCEPTION 'sync_meta.runtime is not configured; run neon-sync migrate with MEMORY_DEVICE_ID';
    END IF;
    event_payload := record_value - 'embedding' - 'fts';
    IF TG_OP = 'DELETE' OR record_value ->> 'storage_tier' <> 'active' THEN
        event_payload := event_payload - 'content' - 'goal' - 'summary';
    END IF;
    INSERT INTO sync_meta.events(event_id, device_id, logical_time, table_name, record_key, operation, payload, payload_checksum)
    VALUES (gen_random_uuid(), device, nextval('sync_meta.logical_clock'), TG_TABLE_NAME, row_key,
        CASE WHEN TG_OP = 'DELETE' THEN 'delete' WHEN record_value ->> 'storage_tier' <> 'active' THEN 'archive' ELSE 'upsert' END,
        event_payload, md5(event_payload::text));
    INSERT INTO sync_meta.outbox(table_name, record_key, generation, operation, changed_at, attempts, last_error)
    VALUES (TG_TABLE_NAME, row_key, 1, CASE WHEN TG_OP = 'DELETE' OR record_value ->> 'storage_tier' <> 'active' THEN 'delete' ELSE 'upsert' END, now(), 0, NULL)
    ON CONFLICT (table_name, record_key) DO UPDATE SET
        generation = sync_meta.outbox.generation + 1,
        operation = EXCLUDED.operation,
        changed_at = EXCLUDED.changed_at,
        attempts = 0,
        last_error = NULL;
    RETURN COALESCE(NEW, OLD);
END;
$$ LANGUAGE plpgsql;
"#).execute(local).await.context("creating local sync queue")?;
    let device_id =
        env::var("MEMORY_DEVICE_ID").context("MEMORY_DEVICE_ID is required for sync migration")?;
    sqlx::query("INSERT INTO sync_meta.runtime(singleton,device_id) VALUES(true,$1) ON CONFLICT(singleton) DO UPDATE SET device_id=EXCLUDED.device_id,updated_at=now()")
        .bind(device_id)
        .execute(local)
        .await?;
    for spec in TABLES {
        let trigger = format!("trg_sync_outbox_{}", spec.name);
        let sql = format!("DROP TRIGGER IF EXISTS {} ON public.{}; CREATE TRIGGER {} AFTER INSERT OR UPDATE OR DELETE ON public.{} FOR EACH ROW EXECUTE FUNCTION sync_meta.capture_change('{}');", quote_ident(&trigger), quote_ident(spec.name), quote_ident(&trigger), quote_ident(spec.name), spec.key);
        sqlx::raw_sql(&sql)
            .execute(local)
            .await
            .with_context(|| format!("installing local outbox trigger for {}", spec.name))?;
    }
    Ok(())
}

async fn ensure_target_meta(neon: &PgPool) -> Result<()> {
    // No local capture function or trigger is installed here. Target metadata is audit-only.
    sqlx::raw_sql(r#"
CREATE SCHEMA IF NOT EXISTS sync_meta;
CREATE TABLE IF NOT EXISTS sync_meta.archive (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(), run_id UUID NOT NULL, table_name TEXT NOT NULL,
    record_key TEXT NOT NULL, row_data JSONB NOT NULL, reason TEXT NOT NULL,
    archived_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS sync_meta.runs (
    run_id UUID PRIMARY KEY, started_at TIMESTAMPTZ NOT NULL DEFAULT now(), finished_at TIMESTAMPTZ,
    status TEXT NOT NULL, rows_applied BIGINT NOT NULL DEFAULT 0, bytes_applied BIGINT NOT NULL DEFAULT 0,
    error_summary TEXT
);
CREATE TABLE IF NOT EXISTS sync_meta.embedding_manifest (
    id UUID PRIMARY KEY, source_table TEXT NOT NULL, source_id UUID NOT NULL, model TEXT, dimension INT,
    created_at TIMESTAMPTZ, UNIQUE(source_table, source_id)
);
CREATE TABLE IF NOT EXISTS sync_meta.leases (
    lease_name TEXT PRIMARY KEY, device_id TEXT NOT NULL, generation BIGINT NOT NULL DEFAULT 1,
    expires_at TIMESTAMPTZ NOT NULL, updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
"#).execute(neon).await.context("creating Neon sync metadata")?;
    let captures: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_trigger t JOIN pg_proc p ON p.oid=t.tgfoid JOIN pg_namespace n ON n.oid=p.pronamespace WHERE NOT t.tgisinternal AND n.nspname='sync_meta' AND p.proname='capture_change'")
        .fetch_one(neon).await?;
    if captures != 0 {
        bail!("unsafe local outbox trigger found on Neon");
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct TargetLease {
    device_id: String,
    generation: i64,
}

async fn acquire_target_lease(neon: &PgPool) -> Result<TargetLease> {
    let device_id = env::var("MEMORY_DEVICE_ID")
        .context("MEMORY_DEVICE_ID is required for shared synchronization")?;
    let row = tokio::time::timeout(QUERY_TIMEOUT, sqlx::query(
        "INSERT INTO sync_meta.leases(lease_name,device_id,generation,expires_at) VALUES('memory-platform-sync',$1,1,now()+interval '10 minutes') \
         ON CONFLICT(lease_name) DO UPDATE SET device_id=EXCLUDED.device_id,generation=sync_meta.leases.generation+1,expires_at=EXCLUDED.expires_at,updated_at=now() \
         WHERE sync_meta.leases.expires_at < now() OR sync_meta.leases.device_id=EXCLUDED.device_id \
         RETURNING generation"
    ).bind(&device_id).fetch_optional(neon))
        .await
        .map_err(|_| anyhow!("target lease acquisition timed out"))??
        .ok_or_else(|| anyhow!("another device currently owns the Neon sync lease"))?;
    // A process can disappear after recording `running`. Once the lease window
    // has elapsed, preserve the durable queue and record that interruption.
    sqlx::query(
        "UPDATE sync_meta.runs SET status='interrupted', finished_at=now(), \
         error_summary=coalesce(error_summary, 'lease expired before completion') \
         WHERE status='running' AND started_at < now() - interval '10 minutes'",
    )
    .execute(neon)
    .await?;
    Ok(TargetLease {
        device_id,
        generation: row.get(0),
    })
}

async fn renew_target_lease(neon: &PgPool, lease: &TargetLease) -> Result<()> {
    let changed = tokio::time::timeout(QUERY_TIMEOUT, sqlx::query(
        "UPDATE sync_meta.leases SET expires_at=now()+interval '10 minutes',updated_at=now() \
         WHERE lease_name='memory-platform-sync' AND device_id=$1 AND generation=$2 AND expires_at > now()"
    ).bind(&lease.device_id).bind(lease.generation).execute(neon))
        .await
        .map_err(|_| anyhow!("target lease renewal timed out"))??
        .rows_affected();
    if changed != 1 {
        bail!("Neon sync lease was lost")
    }
    Ok(())
}

async fn release_target_lease(neon: &PgPool, lease: &TargetLease) {
    let _ = tokio::time::timeout(
        QUERY_TIMEOUT,
        sqlx::query("UPDATE sync_meta.leases SET expires_at=now() WHERE lease_name='memory-platform-sync' AND device_id=$1 AND generation=$2")
            .bind(&lease.device_id)
            .bind(lease.generation)
            .execute(neon),
    )
    .await;
}

async fn record_batch_failure(
    local: &PgPool,
    entries: &[OutboxItem],
    error: &impl std::fmt::Display,
) {
    let message: String = error.to_string().chars().take(500).collect();
    let mut tx = match local.begin().await {
        Ok(tx) => tx,
        Err(_) => return,
    };
    for entry in entries {
        if sqlx::query("UPDATE sync_meta.outbox SET attempts=attempts+1,last_error=$4 WHERE table_name=$1 AND record_key=$2 AND generation=$3")
            .bind(&entry.table)
            .bind(&entry.key)
            .bind(entry.generation)
            .bind(&message)
            .execute(&mut *tx)
            .await
            .is_err()
        {
            return;
        }
    }
    let _ = tx.commit().await;
}

async fn queue(local: &PgPool, spec: TableSpec, key: &str, op: &str) -> Result<()> {
    sqlx::query("INSERT INTO sync_meta.outbox(table_name,record_key,generation,operation,changed_at) VALUES($1,$2,1,$3,now()) ON CONFLICT(table_name,record_key) DO UPDATE SET generation=sync_meta.outbox.generation+1, operation=EXCLUDED.operation, changed_at=EXCLUDED.changed_at, attempts=0,last_error=NULL")
        .bind(spec.name).bind(key).bind(op).execute(local).await?;
    Ok(())
}

async fn push_event_batch(local: &PgPool, neon: &PgPool, lease: &TargetLease) -> Result<usize> {
    let rows = tokio::time::timeout(QUERY_TIMEOUT, sqlx::query(
        "SELECT event_id,device_id,logical_time,table_name,record_key,operation,payload,payload_checksum,supersedes,created_at \
         FROM sync_meta.events WHERE pushed_at IS NULL ORDER BY created_at,event_id LIMIT $1"
    ).bind(DEFAULT_BATCH_ROWS as i64).fetch_all(local))
        .await.map_err(|_| anyhow!("local event read timed out"))??;
    if rows.is_empty() {
        return Ok(0);
    }
    renew_target_lease(neon, lease).await?;
    let mut tx = neon.begin().await?;
    for row in &rows {
        sqlx::query(
            "INSERT INTO sync_meta.events(event_id,device_id,logical_time,table_name,record_key,operation,payload,payload_checksum,supersedes,created_at,received_at) \
             VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,now()) ON CONFLICT(event_id) DO NOTHING"
        )
        .bind(row.try_get::<Uuid, _>(0)?)
        .bind(row.try_get::<String, _>(1)?)
        .bind(row.try_get::<i64, _>(2)?)
        .bind(row.try_get::<String, _>(3)?)
        .bind(row.try_get::<String, _>(4)?)
        .bind(row.try_get::<String, _>(5)?)
        .bind(row.try_get::<Value, _>(6)?)
        .bind(row.try_get::<String, _>(7)?)
        .bind(row.try_get::<Option<Uuid>, _>(8)?)
        .bind(row.try_get::<chrono::DateTime<chrono::Utc>, _>(9)?)
        .execute(&mut *tx).await?;
    }
    tx.commit().await?;
    let ids: Vec<Uuid> = rows
        .iter()
        .map(|r| r.try_get(0))
        .collect::<std::result::Result<_, _>>()?;
    sqlx::query("UPDATE sync_meta.events SET pushed_at=now() WHERE event_id = ANY($1)")
        .bind(ids)
        .execute(local)
        .await?;
    Ok(rows.len())
}

async fn push_events(local: &PgPool, neon: &PgPool, lease: &TargetLease) -> Result<usize> {
    let started = Instant::now();
    let mut published = 0;
    while started.elapsed() < RUN_BUDGET {
        let batch = push_event_batch(local, neon, lease).await?;
        published += batch;
        if batch < DEFAULT_BATCH_ROWS {
            break;
        }
    }
    Ok(published)
}

async fn apply_remote_event(local: &PgPool, table: &str, key: &str, payload: &Value) -> Result<()> {
    let spec = TABLES
        .iter()
        .find(|spec| spec.name == table)
        .ok_or_else(|| anyhow!("remote event references unsupported table {table}"))?;
    if payload.is_null() || payload == &Value::Object(Default::default()) {
        return Ok(());
    }
    let table = quote_ident(spec.name);
    let key_col = quote_ident(spec.key);
    let columns = sqlx::query("SELECT column_name FROM information_schema.columns WHERE table_schema='public' AND table_name=$1 AND is_generated='NEVER' AND column_name NOT IN ('fts','embedding') ORDER BY ordinal_position")
        .bind(spec.name).fetch_all(local).await?;
    let updates: Vec<String> = columns
        .into_iter()
        .map(|row| row.get::<String, _>(0))
        .filter(|column| column != spec.key)
        .map(|column| {
            let col = quote_ident(&column);
            format!("{col}=EXCLUDED.{col}")
        })
        .collect();
    let sql = format!("INSERT INTO public.{table} SELECT (jsonb_populate_record(NULL::public.{table}, $1)).* ON CONFLICT ({key_col}) DO UPDATE SET {}", updates.join(","));
    let mut tx = local.begin().await?;
    sqlx::query("SET LOCAL memory.sync_replay='on'")
        .execute(&mut *tx)
        .await?;
    sqlx::query(&sql).bind(payload).execute(&mut *tx).await?;
    tx.commit().await?;
    let _ = key;
    Ok(())
}

async fn pull_events(local: &PgPool, neon: &PgPool) -> Result<usize> {
    let cursor_time: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT (cursor_value->>'created_at')::timestamptz FROM sync_meta.cursors WHERE cursor_name='neon_pull'"
    ).fetch_optional(local).await?;
    let cursor_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT (cursor_value->>'event_id')::uuid FROM sync_meta.cursors WHERE cursor_name='neon_pull'"
    ).fetch_optional(local).await?;
    let rows = tokio::time::timeout(QUERY_TIMEOUT, sqlx::query(
        "SELECT event_id,device_id,logical_time,table_name,record_key,operation,payload,payload_checksum,supersedes,created_at \
         FROM sync_meta.events WHERE (created_at,event_id) > (COALESCE($1, '-infinity'::timestamptz),COALESCE($2,'00000000-0000-0000-0000-000000000000'::uuid)) ORDER BY created_at,event_id LIMIT $3"
    ).bind(cursor_time).bind(cursor_id).bind(DEFAULT_BATCH_ROWS as i64).fetch_all(neon))
        .await.map_err(|_| anyhow!("remote event read timed out"))??;
    let mut newest = cursor_time.zip(cursor_id);
    let mut imported = 0;
    for row in rows {
        let event_id: Uuid = row.try_get(0)?;
        let inserted = sqlx::query("INSERT INTO sync_meta.events(event_id,device_id,logical_time,table_name,record_key,operation,payload,payload_checksum,supersedes,created_at,received_at,pushed_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,now(),now()) ON CONFLICT(event_id) DO NOTHING")
            .bind(event_id).bind(row.try_get::<String,_>(1)?).bind(row.try_get::<i64,_>(2)?)
            .bind(row.try_get::<String,_>(3)?).bind(row.try_get::<String,_>(4)?).bind(row.try_get::<String,_>(5)?)
            .bind(row.try_get::<Value,_>(6)?).bind(row.try_get::<String,_>(7)?).bind(row.try_get::<Option<Uuid>,_>(8)?)
            .bind(row.try_get::<chrono::DateTime<chrono::Utc>,_>(9)?).execute(local).await?.rows_affected();
        let created: chrono::DateTime<chrono::Utc> = row.try_get(9)?;
        if newest.map_or(true, |current| (created, event_id) > current) {
            newest = Some((created, event_id));
        }
        if inserted == 1 {
            let operation: String = row.try_get(5)?;
            if operation == "upsert" {
                apply_remote_event(
                    local,
                    &row.try_get::<String, _>(3)?,
                    &row.try_get::<String, _>(4)?,
                    &row.try_get::<Value, _>(6)?,
                )
                .await?;
            }
            imported += 1;
        }
    }
    if let Some((created_at, event_id)) = newest {
        sqlx::query("INSERT INTO sync_meta.cursors(cursor_name,cursor_value) VALUES('neon_pull',jsonb_build_object('created_at',$1,'event_id',$2)) ON CONFLICT(cursor_name) DO UPDATE SET cursor_value=EXCLUDED.cursor_value,updated_at=now()")
            .bind(created_at).bind(event_id).execute(local).await?;
    }
    Ok(imported)
}

async fn archive_and_delete_many(
    neon: &PgPool,
    run_id: Uuid,
    spec: TableSpec,
    keys: &[String],
    reason: &str,
) -> Result<()> {
    if keys.is_empty() {
        return Ok(());
    }
    let table = quote_ident(spec.name);
    let column = quote_ident(spec.key);
    let mut tx = neon.begin().await?;
    let archive_sql = format!("INSERT INTO sync_meta.archive(run_id,table_name,record_key,row_data,reason) SELECT $1,$2,{column}::text,to_jsonb(t),$4 FROM public.{table} t WHERE {column}::text = ANY($3)");
    sqlx::query(&archive_sql)
        .bind(run_id)
        .bind(spec.name)
        .bind(keys)
        .bind(reason)
        .execute(&mut *tx)
        .await?;
    let delete_sql = format!("DELETE FROM public.{table} WHERE {column}::text = ANY($1)");
    sqlx::query(&delete_sql)
        .bind(keys)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

async fn columns(neon: &PgPool, spec: TableSpec) -> Result<Vec<String>> {
    let rows = sqlx::query("SELECT column_name FROM information_schema.columns WHERE table_schema='public' AND table_name=$1 AND is_generated='NEVER' AND column_name NOT IN ('fts','embedding') ORDER BY ordinal_position")
        .bind(spec.name).fetch_all(neon).await?;
    Ok(rows.into_iter().map(|r| r.get(0)).collect())
}

async fn source_rows(local: &PgPool, spec: TableSpec, keys: &[String]) -> Result<Vec<SourceRow>> {
    let vector = if spec.has_embedding {
        ", embedding::text"
    } else {
        ""
    };
    let sql = format!("SELECT {}::text, to_jsonb(t) - 'fts' - 'embedding'{} FROM public.{} t WHERE {}::text = ANY($1) AND storage_tier='active'", quote_ident(spec.key), vector, quote_ident(spec.name), quote_ident(spec.key));
    sqlx::query(&sql)
        .bind(keys)
        .fetch_all(local)
        .await?
        .into_iter()
        .map(|row| {
            let vector = if spec.has_embedding {
                row.try_get::<Option<String>, _>(2)?
                    .map(|text| vector_from_text(&text))
                    .transpose()?
            } else {
                None
            };
            Ok(SourceRow {
                key: row.get(0),
                data: row.get(1),
                vector,
            })
        })
        .collect()
}

fn batches(rows: Vec<SourceRow>, max_rows: usize) -> Vec<Vec<SourceRow>> {
    let mut result = Vec::new();
    let mut current = Vec::new();
    let mut bytes = 0;
    for row in rows {
        let row_bytes = serde_json::to_vec(&row.data).map_or(0, |v| v.len())
            + row.vector.as_ref().map_or(0, |v| v.0.len() * 4);
        if !current.is_empty() && (current.len() >= max_rows || bytes + row_bytes > MAX_BATCH_BYTES)
        {
            result.push(current);
            current = Vec::new();
            bytes = 0;
        }
        bytes += row_bytes;
        current.push(row);
    }
    if !current.is_empty() {
        result.push(current);
    }
    result
}

async fn apply_batch(
    neon: &PgPool,
    run_id: Uuid,
    spec: TableSpec,
    cols: &[String],
    rows: &[SourceRow],
) -> Result<usize> {
    let table = quote_ident(spec.name);
    let key = quote_ident(spec.key);
    let updates: Vec<String> = cols
        .iter()
        .filter(|col| *col != spec.key)
        .map(|col| {
            let c = quote_ident(col);
            format!("{c}=EXCLUDED.{c}")
        })
        .collect();
    let insert = format!("INSERT INTO public.{table} SELECT (jsonb_populate_record(NULL::public.{table}, $1)).* ON CONFLICT ({key}) DO UPDATE SET {}", updates.join(","));
    let vector_sql = format!("UPDATE public.{table} SET embedding=$1::vector WHERE {key}::text=$2");
    let mut tx = neon.begin().await?;
    sqlx::query("SET LOCAL memory.sync_replay='on'")
        .execute(&mut *tx)
        .await?;
    for row in rows {
        if let Some(natural_key) = spec.natural_key {
            if let Some(value) = row.data.get(natural_key).and_then(Value::as_str) {
                // A different target ID with the same natural key is stale
                // projection data. Preserve it before the local authority wins.
                let natural = quote_ident(natural_key);
                let archive = format!(
                    "INSERT INTO sync_meta.archive(run_id,table_name,record_key,row_data,reason) \
                     SELECT $1,$2,{key}::text,to_jsonb(t),'natural-key-conflict-replaced-by-local' \
                     FROM public.{table} t WHERE {natural}=$3 AND {key}::text <> $4"
                );
                sqlx::query(&archive)
                    .bind(run_id)
                    .bind(spec.name)
                    .bind(value)
                    .bind(&row.key)
                    .execute(&mut *tx)
                    .await?;
                let delete =
                    format!("DELETE FROM public.{table} WHERE {natural}=$1 AND {key}::text <> $2");
                sqlx::query(&delete)
                    .bind(value)
                    .bind(&row.key)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        sqlx::query(&insert)
            .bind(&row.data)
            .execute(&mut *tx)
            .await?;
        if let Some(vector) = &row.vector {
            sqlx::query(&vector_sql)
                .bind(vector)
                .bind(&row.key)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(rows.len())
}

async fn acknowledge(local: &PgPool, items: &[OutboxItem]) -> Result<()> {
    let mut tx = local.begin().await?;
    for item in items {
        sqlx::query(
            "DELETE FROM sync_meta.outbox WHERE table_name=$1 AND record_key=$2 AND generation=$3",
        )
        .bind(&item.table)
        .bind(&item.key)
        .bind(item.generation)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn run_queue(
    local: &PgPool,
    neon: &PgPool,
    run_id: Uuid,
    lease: &TargetLease,
) -> Result<(usize, usize)> {
    let started = Instant::now();
    let mut applied = 0;
    let mut bytes = 0;
    let mut batch_size = DEFAULT_BATCH_ROWS;
    let mut committed_batches = 0usize;
    let mut consecutive_successes = 0usize;
    while started.elapsed() < RUN_BUDGET {
        renew_target_lease(neon, lease).await?;
        let queue_rows = sqlx::query("SELECT table_name,record_key,generation,operation FROM sync_meta.outbox ORDER BY changed_at LIMIT 200")
            .fetch_all(local).await?;
        if queue_rows.is_empty() {
            break;
        }
        let mut grouped: BTreeMap<String, Vec<OutboxItem>> = BTreeMap::new();
        for row in queue_rows {
            let item = OutboxItem {
                table: row.get(0),
                key: row.get(1),
                generation: row.get(2),
                op: row.get(3),
            };
            grouped.entry(item.table.clone()).or_default().push(item);
        }
        let mut made_progress = false;
        for spec in TABLES {
            let Some(items) = grouped.remove(spec.name) else {
                continue;
            };
            made_progress = true;
            let mut deletes = Vec::new();
            let mut upserts = Vec::new();
            for item in items {
                if item.op == "delete" {
                    deletes.push(item)
                } else {
                    upserts.push(item)
                }
            }
            for delete_batch in deletes.chunks(batch_size) {
                let keys: Vec<String> = delete_batch.iter().map(|item| item.key.clone()).collect();
                archive_and_delete_many(neon, run_id, *spec, &keys, "local-delete").await?;
                acknowledge(local, delete_batch).await?;
                applied += delete_batch.len();
            }
            let cols = columns(neon, *spec).await?;
            let keys: Vec<String> = upserts.iter().map(|i| i.key.clone()).collect();
            let rows = source_rows(local, *spec, &keys).await?;
            let by_key: HashMap<&str, &OutboxItem> =
                upserts.iter().map(|i| (i.key.as_str(), i)).collect();
            // A retry must split the already-materialized payload. Merely reducing
            // `batch_size` here would otherwise retry the same oversized batch.
            let mut pending_batches = batches(rows, batch_size);
            while let Some(batch) = pending_batches.pop() {
                let entries: Vec<OutboxItem> = batch
                    .iter()
                    .filter_map(|r| {
                        by_key.get(r.key.as_str()).map(|i| OutboxItem {
                            table: i.table.clone(),
                            key: i.key.clone(),
                            generation: i.generation,
                            op: i.op.clone(),
                        })
                    })
                    .collect();
                let payload: usize = batch
                    .iter()
                    .map(|r| {
                        serde_json::to_vec(&r.data).map_or(0, |v| v.len())
                            + r.vector.as_ref().map_or(0, |v| v.0.len() * 4)
                    })
                    .sum();
                let mut attempt = 0;
                loop {
                    match apply_batch(neon, run_id, *spec, &cols, &batch).await {
                        Ok(count) => {
                            committed_batches += 1;
                            if env::var("NEON_SYNC_FAIL_AFTER_TARGET_COMMIT")
                                .ok()
                                .and_then(|value| value.parse::<usize>().ok())
                                == Some(committed_batches)
                            {
                                bail!("injected failure after target commit and before local acknowledgement");
                            }
                            acknowledge(local, &entries).await?;
                            applied += count;
                            bytes += payload;
                            consecutive_successes += 1;
                            if consecutive_successes >= 3 && batch_size < DEFAULT_BATCH_ROWS {
                                batch_size += 1;
                                consecutive_successes = 0;
                            }
                            break;
                        }
                        Err(error) if attempt < MAX_RETRIES => {
                            record_batch_failure(local, &entries, &error).await;
                            attempt += 1;
                            consecutive_successes = 0;
                            batch_size = (batch_size / 2).max(MIN_BATCH_ROWS);
                            tokio::time::sleep(Duration::from_millis(250 * (1 << attempt))).await;
                            if attempt == MAX_RETRIES {
                                if batch.len() > MIN_BATCH_ROWS {
                                    let midpoint = batch.len() / 2;
                                    pending_batches.push(batch[midpoint..].to_vec());
                                    pending_batches.push(batch[..midpoint].to_vec());
                                    break;
                                }
                                return Err(error).context("batch retries exhausted for one row");
                            }
                        }
                        Err(error) => return Err(error).context("applying Neon batch"),
                    }
                }
                if started.elapsed() >= RUN_BUDGET {
                    break;
                }
            }
        }
        if !made_progress {
            break;
        }
    }
    Ok((applied, bytes))
}

async fn bootstrap_events(local: &PgPool) -> Result<usize> {
    let device_id =
        env::var("MEMORY_DEVICE_ID").context("MEMORY_DEVICE_ID is required for bootstrap")?;
    let mut created = 0;
    for spec in TABLES {
        let sql = format!(
            "SELECT {}::text, to_jsonb(t) - 'fts' - 'embedding' FROM public.{} t WHERE storage_tier='active' AND NOT EXISTS (SELECT 1 FROM sync_meta.events e WHERE e.table_name=$1 AND e.record_key={}::text AND e.operation='upsert') ORDER BY {} LIMIT $2",
            quote_ident(spec.key), quote_ident(spec.name), quote_ident(spec.key), quote_ident(spec.key)
        );
        let rows = sqlx::query(&sql)
            .bind(spec.name)
            .bind(DEFAULT_BATCH_ROWS as i64)
            .fetch_all(local)
            .await?;
        for row in rows {
            let key: String = row.get(0);
            let payload: Value = row.get(1);
            let inserted = sqlx::query(
                "INSERT INTO sync_meta.events(event_id,device_id,logical_time,table_name,record_key,operation,payload,payload_checksum) \
                 SELECT gen_random_uuid(),$1,nextval('sync_meta.logical_clock'),$2,$3,'upsert',$4,md5($4::text) \
                 WHERE NOT EXISTS (SELECT 1 FROM sync_meta.events WHERE table_name=$2 AND record_key=$3 AND operation='upsert')"
            ).bind(&device_id).bind(spec.name).bind(&key).bind(&payload).execute(local).await?.rows_affected();
            created += inserted as usize;
            // A baseline event alone is not enough: queue the authoritative
            // source row so the active Neon projection advances in the same
            // resumable workflow.
            if inserted == 1 {
                queue(local, *spec, &key, "upsert").await?;
            }
        }
    }
    Ok(created)
}

async fn audit_page(local: &PgPool, neon: &PgPool) -> Result<usize> {
    let state: Option<Value> = sqlx::query_scalar(
        "SELECT cursor_value FROM sync_meta.cursors WHERE cursor_name='audit_page'",
    )
    .fetch_optional(local)
    .await?;
    let table_index = state
        .as_ref()
        .and_then(|v| v.get("table_index"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let after_key = state
        .as_ref()
        .and_then(|v| v.get("after_key"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let spec = TABLES
        .get(table_index % TABLES.len())
        .ok_or_else(|| anyhow!("no sync tables configured"))?;
    let sql = format!("SELECT {}::text, md5((to_jsonb(t) - 'fts' - 'embedding')::text) FROM public.{} t WHERE storage_tier='active' AND {}::text > $1 ORDER BY {} LIMIT $2", quote_ident(spec.key), quote_ident(spec.name), quote_ident(spec.key), quote_ident(spec.key));
    let rows = tokio::time::timeout(
        QUERY_TIMEOUT,
        sqlx::query(&sql)
            .bind(&after_key)
            .bind(DEFAULT_BATCH_ROWS as i64)
            .fetch_all(local),
    )
    .await
    .map_err(|_| anyhow!("local audit page timed out"))??;
    let mut repaired = 0;
    let mut last = None;
    for row in rows {
        let key: String = row.get(0);
        let local_hash: String = row.get(1);
        let remote_sql = format!("SELECT md5((to_jsonb(t) - 'fts' - 'embedding')::text) FROM public.{} t WHERE {}::text=$1", quote_ident(spec.name), quote_ident(spec.key));
        let remote_hash: Option<String> = tokio::time::timeout(
            QUERY_TIMEOUT,
            sqlx::query_scalar(&remote_sql)
                .bind(&key)
                .fetch_optional(neon),
        )
        .await
        .map_err(|_| anyhow!("remote audit lookup timed out"))??;
        if remote_hash.as_deref() != Some(&local_hash) {
            queue(local, *spec, &key, "upsert").await?;
            repaired += 1;
        }
        last = Some(key);
    }
    let next_index = if last.is_none() {
        (table_index + 1) % TABLES.len()
    } else {
        table_index
    };
    sqlx::query("INSERT INTO sync_meta.cursors(cursor_name,cursor_value) VALUES('audit_page',jsonb_build_object('table_index',$1,'after_key',$2)) ON CONFLICT(cursor_name) DO UPDATE SET cursor_value=EXCLUDED.cursor_value,updated_at=now()")
        .bind(next_index as i64).bind(last.unwrap_or_default()).execute(local).await?;
    Ok(repaired)
}

async fn rebuild_derived(local: &PgPool, neon: &PgPool) -> Result<()> {
    let manifest = sqlx::query(
        "SELECT id,source_table,source_id,model,dimension,created_at,embedding::text FROM public.embeddings",
    )
    .fetch_all(local)
    .await?;
    let mut tx = neon.begin().await?;
    for row in &manifest {
        sqlx::query("INSERT INTO sync_meta.embedding_manifest(id,source_table,source_id,model,dimension,created_at) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(source_table,source_id) DO UPDATE SET id=EXCLUDED.id,model=EXCLUDED.model,dimension=EXCLUDED.dimension,created_at=EXCLUDED.created_at")
            .bind(row.try_get::<Uuid,_>(0)?).bind(row.try_get::<String,_>(1)?).bind(row.try_get::<Uuid,_>(2)?).bind(row.try_get::<Option<String>,_>(3)?).bind(row.try_get::<Option<i32>,_>(4)?).bind(row.try_get::<Option<chrono::DateTime<chrono::Utc>>,_>(5)?).execute(&mut *tx).await?;
    }
    sqlx::raw_sql("TRUNCATE public.embeddings; INSERT INTO public.embeddings(id,source_table,source_id,embedding,model,dimension,created_at) SELECT m.id,m.source_table,m.source_id, CASE m.source_table WHEN 'documents' THEN d.embedding WHEN 'memories' THEN me.embedding WHEN 'experiences' THEN e.embedding END, m.model,m.dimension,m.created_at FROM sync_meta.embedding_manifest m LEFT JOIN public.documents d ON m.source_table='documents' AND d.id=m.source_id LEFT JOIN public.memories me ON m.source_table='memories' AND me.id=m.source_id LEFT JOIN public.experiences e ON m.source_table='experiences' AND e.id=m.source_id WHERE CASE m.source_table WHEN 'documents' THEN d.embedding IS NOT NULL WHEN 'memories' THEN me.embedding IS NOT NULL WHEN 'experiences' THEN e.embedding IS NOT NULL ELSE false END;").execute(&mut *tx).await?;
    for row in manifest {
        let source_table: String = row.try_get(1)?;
        if matches!(
            source_table.as_str(),
            "documents" | "memories" | "experiences"
        ) {
            continue;
        }
        // Future source types retain their authoritative cache vector rather
        // than being silently dropped by the known-source reconstruction.
        let vector = vector_from_text(&row.try_get::<String, _>(6)?)?;
        sqlx::query("INSERT INTO public.embeddings(id,source_table,source_id,embedding,model,dimension,created_at) VALUES($1,$2,$3,$4,$5,$6,$7)")
            .bind(row.try_get::<Uuid,_>(0)?)
            .bind(source_table)
            .bind(row.try_get::<Uuid,_>(2)?)
            .bind(vector)
            .bind(row.try_get::<Option<String>,_>(3)?)
            .bind(row.try_get::<Option<i32>,_>(4)?)
            .bind(row.try_get::<Option<chrono::DateTime<chrono::Utc>>,_>(5)?)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::raw_sql("UPDATE public.documents SET content=content; UPDATE public.memories SET content=content; UPDATE public.experiences SET goal=goal;").execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

async fn status(local: &PgPool, neon: &PgPool) -> Result<()> {
    let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM sync_meta.outbox")
        .fetch_one(local)
        .await?;
    println!("pending queue entries: {pending}");
    let latest_run = sqlx::query(
        "SELECT status,started_at,finished_at,rows_applied,bytes_applied,error_summary FROM sync_meta.runs ORDER BY started_at DESC LIMIT 1",
    )
    .fetch_optional(neon)
    .await?;
    if let Some(row) = latest_run {
        println!(
            "last run: status={} started={} finished={} batch_rows={} bytes={} replay_state={}",
            row.try_get::<String, _>(0)?,
            row.try_get::<chrono::DateTime<chrono::Utc>, _>(1)?,
            row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(2)?
                .map(|v| v.to_rfc3339())
                .unwrap_or_else(|| "-".into()),
            row.try_get::<i64, _>(3).unwrap_or_default(),
            row.try_get::<i64, _>(4).unwrap_or_default(),
            row.try_get::<Option<String>, _>(5)?
                .unwrap_or_else(|| "clean".into()),
        );
    }
    let state = sqlx::query(
        "SELECT state_key,state_value,updated_at FROM sync_meta.state ORDER BY state_key",
    )
    .fetch_all(local)
    .await?;
    for row in state {
        let key: String = row.get(0);
        let value: Value = row.get(1);
        let updated: chrono::DateTime<chrono::Utc> = row.get(2);
        println!("state {key} at {updated}: {value}");
    }
    for spec in TABLES {
        let local_count: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM public.{}",
            quote_ident(spec.name)
        ))
        .fetch_one(local)
        .await?;
        let neon_count: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM public.{}",
            quote_ident(spec.name)
        ))
        .fetch_one(neon)
        .await?;
        println!("{}: local={local_count} neon={neon_count}", spec.name);
    }
    Ok(())
}

async fn health(local: &PgPool, neon: &PgPool) -> Result<()> {
    let local_identity = sqlx::query("SELECT current_database(), COALESCE(inet_server_addr()::text, 'local'), COALESCE(inet_server_port(), 0)").fetch_one(local).await?;
    let neon_identity = sqlx::query("SELECT current_database(), COALESCE(inet_server_addr()::text, 'remote'), COALESCE(inet_server_port(), 0)").fetch_one(neon).await?;
    let local_migrations: i64 = sqlx::query_scalar("SELECT count(*) FROM _migrations")
        .fetch_one(local)
        .await?;
    let neon_migrations: i64 = sqlx::query_scalar("SELECT count(*) FROM _migrations")
        .fetch_one(neon)
        .await?;
    println!(
        "local database={} host={} port={} migrations={}",
        local_identity.get::<String, _>(0),
        local_identity.get::<String, _>(1),
        local_identity.get::<i32, _>(2),
        local_migrations
    );
    println!(
        "neon database={} host={} port={} migrations={}",
        neon_identity.get::<String, _>(0),
        neon_identity.get::<String, _>(1),
        neon_identity.get::<i32, _>(2),
        neon_migrations
    );
    Ok(())
}

async fn push_once(local: &PgPool, neon: &PgPool) -> Result<(usize, usize, usize)> {
    let lease = acquire_target_lease(neon).await?;
    let run = Uuid::new_v4();
    sqlx::query("INSERT INTO sync_meta.runs(run_id,status) VALUES($1,'running')")
        .bind(run)
        .execute(neon)
        .await?;
    let result = async {
        let event_count = push_events(local, neon, &lease).await?;
        let (rows, bytes) = run_queue(local, neon, run, &lease).await?;
        sqlx::query("INSERT INTO sync_meta.state(state_key,state_value) VALUES('last_success',jsonb_build_object('at',now(),'rows',$1,'bytes',$2)) ON CONFLICT(state_key) DO UPDATE SET state_value=EXCLUDED.state_value,updated_at=now()")
            .bind(rows as i64)
            .bind(bytes as i64)
            .execute(local)
            .await?;
        Ok::<_, anyhow::Error>((rows, bytes, event_count))
    }
    .await;
    release_target_lease(neon, &lease).await;
    match result {
        Ok((rows, bytes, event_count)) => {
            sqlx::query("UPDATE sync_meta.runs SET status='success',finished_at=now(),rows_applied=$2,bytes_applied=$3 WHERE run_id=$1")
                .bind(run)
                .bind(rows as i64)
                .bind(bytes as i64)
                .execute(neon)
                .await?;
            Ok((rows, bytes, event_count))
        }
        Err(error) => {
            let _ = sqlx::query("UPDATE sync_meta.runs SET status='failed',finished_at=now(),error_summary=$2 WHERE run_id=$1")
                .bind(run)
                .bind(error.to_string())
                .execute(neon)
                .await;
            Err(error)
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();
    let (local_url, neon_url) = urls(&cli)?;
    let local = connect(&local_url, "local PostgreSQL").await?;
    let neon = connect(&neon_url, "Neon").await?;
    match cli.command {
        Command::Status => status(&local, &neon).await,
        Command::Health => health(&local, &neon).await,
        Command::Migrate => {
            Migrator::run(&local).await?;
            Migrator::run(&neon).await?;
            ensure_local_queue(&local).await?;
            ensure_target_meta(&neon).await?;
            println!("migrations and local capture triggers are installed");
            Ok(())
        }
        Command::Reconcile | Command::Audit => {
            let repaired = audit_page(&local, &neon).await?;
            println!("audit queued {repaired} local repair(s); no target rows were deleted");
            Ok(())
        }
        Command::Bootstrap => {
            let created = bootstrap_events(&local).await?;
            println!("created {created} baseline event(s)");
            Ok(())
        }
        Command::Full { confirm_full_push } => {
            if !confirm_full_push {
                bail!("refusing full recovery; pass --confirm-full-push explicitly");
            }
            let mut pages = 0usize;
            loop {
                let created = bootstrap_events(&local).await?;
                let (rows, bytes, events) = push_once(&local, &neon).await?;
                pages += 1;
                println!("full page {pages}: baseline={created} rows={rows} events={events} bytes={bytes}");
                let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM sync_meta.outbox")
                    .fetch_one(&local)
                    .await?;
                if created == 0 && remaining == 0 {
                    break;
                }
            }
            println!("full active-corpus recovery complete after {pages} page(s)");
            Ok(())
        }
        Command::Pull => {
            let imported = pull_events(&local, &neon).await?;
            println!("pulled {imported} remote event(s)");
            Ok(())
        }
        Command::RebuildDerived => rebuild_derived(&local, &neon).await,
        Command::Run | Command::Push => {
            let (rows, bytes, event_count) = push_once(&local, &neon).await?;
            println!(
                "sync complete: {rows} projection rows, {event_count} event(s), {bytes} bytes"
            );
            Ok(())
        }
        Command::ResetTarget { confirm_neon_reset } => {
            if !confirm_neon_reset {
                bail!("refusing destructive reset; pass --confirm-neon-reset explicitly");
            }
            sqlx::raw_sql("DROP SCHEMA public CASCADE; CREATE SCHEMA public; GRANT ALL ON SCHEMA public TO PUBLIC;").execute(&neon).await?;
            Migrator::run(&neon).await?;
            ensure_target_meta(&neon).await?;
            println!("Neon target reset; run neon-sync reconcile next");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vector_binary_is_dimension_then_floats() {
        assert_eq!(PgVector(vec![1.0, 2.0]).size_hint(), 12);
    }
    #[test]
    fn batch_limits_rows_and_bytes() {
        let rows = (0..26)
            .map(|i| SourceRow {
                key: i.to_string(),
                data: Value::String("x".repeat(10)),
                vector: None,
            })
            .collect();
        assert_eq!(batches(rows, 25).len(), 2);
    }
    #[test]
    fn table_order_has_no_derived_cache() {
        assert!(!TABLES.iter().any(|table| table.name == "embeddings"));
    }
}
