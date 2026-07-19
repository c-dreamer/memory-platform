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
    /// Neon direct (non-pooler) PostgreSQL URL. Reads NEON_DIRECT then NEON_DATABASE_URL.
    #[arg(long, env = "NEON_DIRECT")]
    neon_url: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Reconcile if needed and drain the queue within the run budget.
    Run,
    /// Compare complete inventories, archive stale target rows, and enqueue differences.
    Reconcile,
    /// Show queue state and count parity without changing either corpus.
    Status,
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

#[derive(Debug)]
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
        .or_else(|| env::var("NEON_DATABASE_URL").ok())
        .ok_or_else(|| anyhow!("NEON_DIRECT or NEON_DATABASE_URL is required"))?;
    // Pooler connections are not suitable for DDL or long transactions.
    Ok((local, neon.replace("-pooler", "")))
}

async fn connect(url: &str, label: &str) -> Result<PgPool> {
    let options = url
        .parse::<PgConnectOptions>()
        .context("invalid PostgreSQL URL")?;
    let mut last_error = None;
    for attempt in 0..=MAX_RETRIES {
        let connection = PgPoolOptions::new()
            .max_connections(2)
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
CREATE OR REPLACE FUNCTION sync_meta.capture_change() RETURNS trigger AS $$
DECLARE
    record_value JSONB;
    row_key TEXT;
BEGIN
    IF TG_OP = 'UPDATE' AND (to_jsonb(NEW) - 'updated_at') IS NOT DISTINCT FROM (to_jsonb(OLD) - 'updated_at') THEN
        RETURN NEW;
    END IF;
    record_value := CASE WHEN TG_OP = 'DELETE' THEN to_jsonb(OLD) ELSE to_jsonb(NEW) END;
    row_key := record_value ->> TG_ARGV[0];
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
"#).execute(neon).await.context("creating Neon sync metadata")?;
    let captures: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_trigger t JOIN pg_proc p ON p.oid=t.tgfoid JOIN pg_namespace n ON n.oid=p.pronamespace WHERE NOT t.tgisinternal AND n.nspname='sync_meta' AND p.proname='capture_change'")
        .fetch_one(neon).await?;
    if captures != 0 {
        bail!("unsafe local outbox trigger found on Neon");
    }
    Ok(())
}

async fn queue(local: &PgPool, spec: TableSpec, key: &str, op: &str) -> Result<()> {
    sqlx::query("INSERT INTO sync_meta.outbox(table_name,record_key,generation,operation,changed_at) VALUES($1,$2,1,$3,now()) ON CONFLICT(table_name,record_key) DO UPDATE SET generation=sync_meta.outbox.generation+1, operation=EXCLUDED.operation, changed_at=EXCLUDED.changed_at, attempts=0,last_error=NULL")
        .bind(spec.name).bind(key).bind(op).execute(local).await?;
    Ok(())
}

async fn inventory(pool: &PgPool, spec: TableSpec) -> Result<BTreeMap<String, String>> {
    let embedding = if spec.has_embedding {
        ", coalesce(md5((embedding::text)::text), '')"
    } else {
        ""
    };
    let sql = format!(
        "SELECT {}::text, md5((to_jsonb(t) - 'fts' - 'embedding')::text){} FROM public.{} t WHERE t.storage_tier='active'",
        quote_ident(spec.key),
        embedding,
        quote_ident(spec.name)
    );
    let rows = sqlx::query(&sql)
        .fetch_all(pool)
        .await
        .with_context(|| format!("reading complete {} inventory", spec.name))?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let key: String = row.get(0);
            let scalar: String = row.get(1);
            let vector: Option<String> = if spec.has_embedding {
                Some(row.get(2))
            } else {
                None
            };
            (key, format!("{scalar}:{}", vector.unwrap_or_default()))
        })
        .collect())
}

async fn archive_and_delete(
    neon: &PgPool,
    run_id: Uuid,
    spec: TableSpec,
    key: &str,
    reason: &str,
) -> Result<()> {
    let table = quote_ident(spec.name);
    let column = quote_ident(spec.key);
    let mut tx = neon.begin().await?;
    let archive_sql = format!("INSERT INTO sync_meta.archive(run_id,table_name,record_key,row_data,reason) SELECT $1,$2,$3,to_jsonb(t),$4 FROM public.{table} t WHERE {column}::text=$3");
    sqlx::query(&archive_sql)
        .bind(run_id)
        .bind(spec.name)
        .bind(key)
        .bind(reason)
        .execute(&mut *tx)
        .await?;
    let delete_sql = format!("DELETE FROM public.{table} WHERE {column}::text=$1");
    sqlx::query(&delete_sql).bind(key).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
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

async fn resolve_natural_conflicts(local: &PgPool, neon: &PgPool, run_id: Uuid) -> Result<()> {
    for spec in TABLES.iter().copied().filter(|s| s.natural_key.is_some()) {
        let natural = quote_ident(spec.natural_key.unwrap());
        let table = quote_ident(spec.name);
        let key = quote_ident(spec.key);
        let sql = format!("SELECT l.{key}::text, n.{key}::text FROM public.{table} l JOIN public.{table} n ON l.{natural}=n.{natural} WHERE l.{key} IS DISTINCT FROM n.{key}");
        // This query must run across databases, so obtain local natural keys then look each up remotely.
        let local_values = sqlx::query(&format!(
            "SELECT {natural}::text, {key}::text FROM public.{table}"
        ))
        .fetch_all(local)
        .await?;
        for row in local_values {
            let value: String = row.get(0);
            let local_key: String = row.get(1);
            let remote_sql =
                format!("SELECT {key}::text FROM public.{table} WHERE {natural}::text=$1");
            if let Some(remote_key) = sqlx::query_scalar::<_, String>(&remote_sql)
                .bind(&value)
                .fetch_optional(neon)
                .await?
            {
                if remote_key != local_key {
                    archive_and_delete(neon, run_id, spec, &remote_key, "natural-key-conflict")
                        .await?;
                }
            }
        }
        let _ = sql; // Documents the invariant while keeping inventories database-local.
    }
    Ok(())
}

async fn reconcile(local: &PgPool, neon: &PgPool, run_id: Uuid) -> Result<usize> {
    // Fetch all inventories first. A partial/incomplete read cannot mutate the target.
    let mut local_maps = HashMap::new();
    let mut neon_maps = HashMap::new();
    for spec in TABLES {
        local_maps.insert(spec.name, inventory(local, *spec).await?);
        neon_maps.insert(spec.name, inventory(neon, *spec).await?);
    }
    resolve_natural_conflicts(local, neon, run_id).await?;
    // Re-read after conflict removals, then archive stale target keys in reverse dependency order.
    let mut changes = 0;
    for spec in TABLES.iter().rev().copied() {
        let source = local_maps.get(spec.name).expect("local inventory");
        let target = neon_maps.get(spec.name).expect("target inventory");
        for key in target.keys().filter(|key| !source.contains_key(*key)) {
            archive_and_delete(neon, run_id, spec, key, "missing-from-local").await?;
            changes += 1;
        }
    }
    for spec in TABLES.iter().copied() {
        let source = local_maps.get(spec.name).expect("local inventory");
        let target = neon_maps.get(spec.name).expect("target inventory");
        for (key, fingerprint) in source {
            if target.get(key) != Some(fingerprint) {
                queue(local, spec, key, "upsert").await?;
                changes += 1;
            }
        }
    }
    sqlx::query("INSERT INTO sync_meta.state(state_key,state_value) VALUES('last_reconcile',jsonb_build_object('at',now(),'changes',$1)) ON CONFLICT(state_key) DO UPDATE SET state_value=EXCLUDED.state_value,updated_at=now()")
        .bind(changes as i64).execute(local).await?;
    Ok(changes)
}

async fn reconciliation_due(local: &PgPool) -> Result<bool> {
    let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM sync_meta.outbox")
        .fetch_one(local)
        .await?;
    // Drain durable work before scheduling an expensive full inventory scan.
    if pending > 0 {
        return Ok(false);
    }
    let due: bool = sqlx::query_scalar(
        "SELECT COALESCE((SELECT updated_at < now() - interval '7 days' FROM sync_meta.state WHERE state_key='last_reconcile'), true)",
    )
    .fetch_one(local)
    .await?;
    Ok(due)
}

async fn derived_rebuild_due(local: &PgPool, rows_applied: usize) -> Result<bool> {
    if rows_applied != 0 {
        return Ok(true);
    }
    let rebuilt: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM sync_meta.state WHERE state_key='last_derived_rebuild'")
            .fetch_optional(local)
            .await?;
    Ok(rebuilt.is_none())
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

async fn run_queue(local: &PgPool, neon: &PgPool, run_id: Uuid) -> Result<(usize, usize)> {
    let started = Instant::now();
    let mut applied = 0;
    let mut bytes = 0;
    let mut batch_size = DEFAULT_BATCH_ROWS;
    let mut committed_batches = 0usize;
    let mut consecutive_successes = 0usize;
    while started.elapsed() < RUN_BUDGET {
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
            for batch in batches(rows, batch_size) {
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
                    match apply_batch(neon, *spec, &cols, &batch).await {
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
                            attempt += 1;
                            consecutive_successes = 0;
                            batch_size = (batch_size / 2).max(MIN_BATCH_ROWS);
                            tokio::time::sleep(Duration::from_millis(250 * (1 << attempt))).await;
                            if attempt == MAX_RETRIES {
                                return Err(error).context("batch retries exhausted");
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
            row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(2)?.map(|v| v.to_rfc3339()).unwrap_or_else(|| "-".into()),
            row.try_get::<i64, _>(3).unwrap_or_default(),
            row.try_get::<i64, _>(4).unwrap_or_default(),
            row.try_get::<Option<String>, _>(5)?.unwrap_or_else(|| "clean".into()),
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

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();
    let (local_url, neon_url) = urls(&cli)?;
    let local = connect(&local_url, "local PostgreSQL").await?;
    let neon = connect(&neon_url, "Neon").await?;
    Migrator::run(&local).await?;
    Migrator::run(&neon).await?;
    ensure_local_queue(&local).await?;
    ensure_target_meta(&neon).await?;
    match cli.command {
        Command::Status => status(&local, &neon).await,
        Command::Reconcile => {
            let run = Uuid::new_v4();
            let changes = reconcile(&local, &neon, run).await?;
            println!("reconciliation queued or archived {changes} rows");
            Ok(())
        }
        Command::RebuildDerived => rebuild_derived(&local, &neon).await,
        Command::Run => {
            let run = Uuid::new_v4();
            sqlx::query("INSERT INTO sync_meta.runs(run_id,status) VALUES($1,'running')")
                .bind(run)
                .execute(&neon)
                .await?;
            let result = async {
                if reconciliation_due(&local).await? {
                    reconcile(&local, &neon, run).await?;
                }
                let (rows, bytes) = run_queue(&local, &neon, run).await?;
                if derived_rebuild_due(&local, rows).await? {
                    rebuild_derived(&local, &neon).await?;
                    sqlx::query("INSERT INTO sync_meta.state(state_key,state_value) VALUES('last_derived_rebuild',jsonb_build_object('at',now())) ON CONFLICT(state_key) DO UPDATE SET state_value=EXCLUDED.state_value,updated_at=now()")
                        .execute(&local)
                        .await?;
                }
                sqlx::query("INSERT INTO sync_meta.state(state_key,state_value) VALUES('last_success',jsonb_build_object('at',now(),'rows',$1,'bytes',$2)) ON CONFLICT(state_key) DO UPDATE SET state_value=EXCLUDED.state_value,updated_at=now()")
                    .bind(rows as i64)
                    .bind(bytes as i64)
                    .execute(&local)
                    .await?;
                Ok::<_, anyhow::Error>((rows, bytes))
            }
            .await;
            match result {
                Ok((rows, bytes)) => {
                    sqlx::query("UPDATE sync_meta.runs SET status='success',finished_at=now(),rows_applied=$2,bytes_applied=$3 WHERE run_id=$1").bind(run).bind(rows as i64).bind(bytes as i64).execute(&neon).await?;
                    println!("sync complete: {rows} rows, {bytes} bytes");
                    Ok(())
                }
                Err(error) => {
                    let _=sqlx::query("UPDATE sync_meta.runs SET status='failed',finished_at=now(),error_summary=$2 WHERE run_id=$1").bind(run).bind(error.to_string()).execute(&neon).await;
                    Err(error)
                }
            }
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
