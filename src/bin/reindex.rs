//! One-shot backfill: re-embed all documents with NULL embeddings via NVIDIA API.

use anyhow::{Context, Result};
use memory_platform::services::embedding::{EmbeddingConfig, EmbeddingService, EmbeddingServiceFactory};
use memory_platform::services::ingestion::IngestionService;
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

    let db_url = std::env::var("DATABASE_URL")
        .context("DATABASE_URL is required")?;
    let nvidia_api_key = std::env::var("NVIDIA_API_KEY")
        .context("NVIDIA_API_KEY is required")?;
    let nvidia_api_url = std::env::var("NVIDIA_API_URL")
        .unwrap_or_else(|_| "https://integrate.api.nvidia.com/v1/embeddings".into());

    info!("Connecting to database...");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .context("Failed to connect to PostgreSQL")?;

    let embedding_config = EmbeddingConfig {
        model: "nvidia".into(),
        nvidia_api_url: Some(nvidia_api_url),
        nvidia_api_key: Some(nvidia_api_key),
        nvidia_embedding_model: "nvidia/nv-embed-v1".into(),
        cache_size: 1000,
    };

    info!("Initializing NVIDIA embedding service...");
    let embedder: Arc<dyn EmbeddingService> = Arc::new(
        EmbeddingServiceFactory::new(embedding_config).await?
    );

    let ingestion = IngestionService::new(pool, embedder);

    info!("Re-indexing documents with null embeddings...");
    let report = ingestion.reindex_all("backfill").await?;

    println!("\n=== Reindex Complete ===");
    println!("  Scanned:   {}", report.files_scanned);
    println!("  Re-indexed: {}", report.files_ingested);
    println!("  Chunks:    {}", report.chunks_created);
    println!("  Errors:    {}", report.errors.len());
    for err in &report.errors {
        println!("    - {}", err);
    }
    println!("  Duration:  {}ms", report.duration_ms);

    Ok(())
}
