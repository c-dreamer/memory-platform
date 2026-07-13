//! MCP stdio server — standalone binary entry point.
//!
//! Implements the Model Context Protocol over stdin/stdout.

use std::sync::Arc;

use memory_platform::config::Config;
use memory_platform::db::postgres::PostgresDb;
use memory_platform::mcp;
use memory_platform::migrations::Migrator;
use memory_platform::search::SearchEngine;
use memory_platform::services::context::ContextService;
use memory_platform::services::decay::DecayEngine;
use memory_platform::services::embedding::{
    EmbeddingConfig, EmbeddingService, EmbeddingServiceFactory,
};
use memory_platform::services::experience::ExperienceService;
use memory_platform::services::procedure::ProcedureService;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    tracing::info!("MCP server starting");

    // Load config from environment
    let config = Arc::new(Config::from_env()?);
    tracing::info!("Loaded config");

    // Connect to PostgreSQL, but keep the MCP server alive in degraded mode if
    // the database is temporarily unreachable. Codex can still complete the
    // handshake and use any tools that do not need live storage.
    let db = match PostgresDb::connect(config.as_ref()).await {
        Ok(db) => {
            tracing::info!("Connected to PostgreSQL");
            let db = Arc::new(db);
            Migrator::run(&db.pool).await?;
            tracing::info!("Database migrations completed");
            db
        }
        Err(e) => {
            tracing::warn!("PostgreSQL unavailable, starting MCP in degraded mode: {e}");
            Arc::new(PostgresDb::new_empty())
        }
    };

    // Clone pool for services that need PgPool directly
    let pool = db.pool.clone();

    // Initialize services
    let search = Arc::new(SearchEngine::new(Arc::clone(&db), Arc::clone(&config)));

    // Embedding service is optional — tools fall back to keyword search if unavailable
    let embedding_service: Option<Arc<dyn EmbeddingService>> = {
        let embedding_config = EmbeddingConfig {
            model: config.embedding_model.clone(),
            nvidia_api_url: Some(config.nvidia_api_url.clone()),
            nvidia_api_key: Some(config.nvidia_api_key.clone()),
            nvidia_embedding_model: config.nvidia_embedding_model.clone(),
            cache_size: config.embedding_cache_size,
        };
        match EmbeddingServiceFactory::new(embedding_config).await {
            Ok(svc) => Some(Arc::new(svc)),
            Err(e) => {
                tracing::warn!("Embedding service unavailable (keyword-only fallback): {e}");
                None
            }
        }
    };

    let context_service = Arc::new(ContextService::new(pool.clone(), Arc::clone(&search)));
    let decay_engine = Arc::new(DecayEngine::new(Arc::clone(&config)));
    let experience_service = Arc::new(ExperienceService::new(
        pool.clone(),
        Arc::clone(&search),
        embedding_service.clone(),
    ));
    let procedure_service = Arc::new(ProcedureService::new(pool.clone()));

    // Build AppState
    let state = Arc::new(memory_platform::AppState {
        config: (*config).clone(),
        db: Arc::clone(&db),
        search,
        neo4j_client: None,
        redis_cache: None,
        context_service: Some(context_service),
        contradiction_detector: None,
        decay_engine: Some(decay_engine),
        embedding_service,
        experience_service: Some(experience_service),
        ingestion_service: None,
        procedure_service: Some(procedure_service),
    });

    // Create MCP server
    let server = mcp::McpServer::new(state);
    tracing::info!("MCP server ready");

    // Enter JSON-RPC listen loop
    server
        .listen(tokio::io::stdin(), tokio::io::stdout())
        .await?;

    Ok(())
}
