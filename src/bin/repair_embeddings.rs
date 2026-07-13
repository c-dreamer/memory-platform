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
        cache_size: config.embedding_cache_size,
    };

    info!("Initializing embedding service...");
    let embedder: Arc<dyn EmbeddingService> =
        Arc::new(EmbeddingServiceFactory::new(embedding_config).await?);

    let ingestion = IngestionService::new(pool, embedder);

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

    Ok(())
}
