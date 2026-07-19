//! Repair null embeddings across operational tables.

use anyhow::{Context, Result};
use memory_platform::services::embedding::{
    EmbeddingConfig, EmbeddingService, EmbeddingServiceFactory,
};
use memory_platform::services::ingestion::IngestionService;
use memory_platform::Config;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    let config = Config::from_env().context("Failed to load config")?;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&config.database_url)
        .await
        .context("Failed to connect to PostgreSQL")?;

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
    for table in ["documents", "memories", "experiences", "sessions", "summaries", "code_changes", "trading_results"] {
        let nulls: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE embedding IS NULL"))
            .fetch_one(&pool).await?;
        let bad_dims: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE embedding IS NOT NULL AND vector_dims(embedding) <> 2048"))
            .fetch_one(&pool).await?;
        remaining_nulls += nulls;
        mismatched += bad_dims;
        println!("  {table}: null_embeddings={nulls} mismatched_dimensions={bad_dims}");
    }
    let cache_bad: i64 = sqlx::query_scalar("SELECT count(*) FROM embeddings WHERE embedding IS NULL OR vector_dims(embedding) <> 2048")
        .fetch_one(&pool).await?;
    mismatched += cache_bad;
    println!("  embeddings cache: invalid_rows={cache_bad}");

    if !report.errors.is_empty() || remaining_nulls != 0 || mismatched != 0 {
        anyhow::bail!("embedding repair incomplete: {} provider/database errors, {remaining_nulls} null embeddings, {mismatched} invalid vectors", report.errors.len());
    }

    Ok(())
}
