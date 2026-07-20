//! Repair null embeddings across operational tables.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use memory_platform::services::embedding::{
    EmbeddingConfig, EmbeddingService, EmbeddingServiceFactory,
};
use memory_platform::services::ingestion::IngestionService;
use memory_platform::Config;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use tracing::info;
use tracing_subscriber::EnvFilter;

const EMBEDDING_TABLES: &[&str] = &[
    "documents",
    "memories",
    "experiences",
    "sessions",
    "summaries",
    "code_changes",
    "trading_results",
];

#[derive(Parser)]
#[command(
    name = "repair-embeddings",
    about = "Repair and verify 2048-dimensional embeddings"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Clone, Copy)]
enum Command {
    /// Repair eligible null embeddings and fail unless verification passes.
    Run,
    /// Print current embedding progress as JSON without starting a provider.
    Status,
    /// Verify null counts, dimensions, and derived cache integrity.
    Verify,
}

async fn connect(config: &Config) -> Result<sqlx::PgPool> {
    PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(std::time::Duration::from_secs(60))
        .connect(&config.database_url)
        .await
        .context("Failed to connect to PostgreSQL")
}

async fn embedding_status(pool: &sqlx::PgPool) -> Result<serde_json::Value> {
    let mut tables = serde_json::Map::new();
    let mut nulls = 0_i64;
    let mut mismatched = 0_i64;
    let mut rows = 0_i64;
    for table in EMBEDDING_TABLES {
        let sql = format!(
            "SELECT count(*)::bigint, count(*) FILTER (WHERE embedding IS NULL)::bigint, count(*) FILTER (WHERE embedding IS NOT NULL AND vector_dims(embedding) <> 2048)::bigint FROM {table}"
        );
        let (total, table_nulls, table_bad): (i64, i64, i64) = sqlx::query_as(&sql)
            .fetch_one(pool)
            .await
            .with_context(|| format!("checking {table} embeddings"))?;
        rows += total;
        nulls += table_nulls;
        mismatched += table_bad;
        tables.insert(
            (*table).to_string(),
            serde_json::json!({"rows": total, "null": table_nulls, "invalid_dimension": table_bad}),
        );
    }
    let cache_invalid: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM embeddings WHERE embedding IS NULL OR vector_dims(embedding) <> 2048",
    )
    .fetch_one(pool)
    .await?;
    mismatched += cache_invalid;
    Ok(serde_json::json!({
        "status": if nulls == 0 && mismatched == 0 { "succeeded" } else { "incomplete" },
        "expected_dimension": 2048,
        "rows": rows,
        "null_embeddings": nulls,
        "invalid_dimensions": mismatched,
        "derived_cache_invalid": cache_invalid,
        "tables": tables,
    }))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let command = cli.command.unwrap_or(Command::Run);
    let config = Config::from_env().context("Failed to load config")?;
    let pool = connect(&config).await?;

    if matches!(command, Command::Status | Command::Verify) {
        let status = embedding_status(&pool).await?;
        println!("{}", serde_json::to_string_pretty(&status)?);
        if matches!(command, Command::Verify) && status["status"].as_str() != Some("succeeded") {
            anyhow::bail!("embedding verification incomplete: {}", status);
        }
        return Ok(());
    }

    let embedding_config = EmbeddingConfig {
        model: config.embedding_model.clone(),
        nvidia_api_url: Some(config.nvidia_api_url.clone()),
        nvidia_api_key: if config.nvidia_api_key.is_empty() {
            None
        } else {
            Some(config.nvidia_api_key.clone())
        },
        nvidia_embedding_model: config.nvidia_embedding_model.clone(),
        expected_dimension: config.embedding_dim,
        cache_size: config.embedding_cache_size,
    };

    info!("Initializing embedding service...");
    let embedder: Arc<dyn EmbeddingService> =
        Arc::new(EmbeddingServiceFactory::new(embedding_config).await?);

    let ingestion = IngestionService::new(pool.clone(), embedder);

    info!("Repairing null embeddings across memories, experiences, and sessions...");
    let report = ingestion.repair_null_embeddings().await?;

    println!("\n=== Repair Complete ===");
    println!("  Scanned:   {}", report.files_scanned);
    println!("  Fixed:     {}", report.files_ingested);
    println!("  Errors:    {}", report.errors.len());
    for err in &report.errors {
        println!("    - {}", err);
    }
    println!("  Duration:  {}ms", report.duration_ms);

    let mut remaining_nulls = 0_i64;
    let mut mismatched = 0_i64;
    for table in EMBEDDING_TABLES {
        let nulls: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM {table} WHERE embedding IS NULL"
        ))
        .fetch_one(&pool)
        .await?;
        let bad_dims: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE embedding IS NOT NULL AND vector_dims(embedding) <> 2048"))
            .fetch_one(&pool).await?;
        remaining_nulls += nulls;
        mismatched += bad_dims;
        println!("  {table}: null_embeddings={nulls} mismatched_dimensions={bad_dims}");
    }
    let cache_bad: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM embeddings WHERE embedding IS NULL OR vector_dims(embedding) <> 2048",
    )
    .fetch_one(&pool)
    .await?;
    mismatched += cache_bad;
    println!("  embeddings cache: invalid_rows={cache_bad}");

    if !report.errors.is_empty() || remaining_nulls != 0 || mismatched != 0 {
        anyhow::bail!("embedding repair incomplete: {} provider/database errors, {remaining_nulls} null embeddings, {mismatched} invalid vectors", report.errors.len());
    }

    Ok(())
}
