//! Memory Platform — main binary entry point.
//!
//! Starts the Axum HTTP server on the configured port.
//! Initializes all services, database connections, and migration runner.

use std::sync::Arc;

use memory_platform::api;
use memory_platform::config::Config;
use memory_platform::db::neo4j::GraphClient;
use memory_platform::db::postgres::PostgresDb;
use memory_platform::db::redis::RedisCache;
use memory_platform::migrations::Migrator;
use memory_platform::search::SearchEngine;
use memory_platform::services::context::ContextService;
use memory_platform::services::contradiction::ContradictionDetector;
use memory_platform::services::decay::DecayEngine;
use memory_platform::services::embedding::{EmbeddingConfig, EmbeddingServiceFactory};
use memory_platform::services::experience::ExperienceService;
use memory_platform::services::ingestion::IngestionService;
use memory_platform::services::procedure::ProcedureService;
use memory_platform::AppState;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(fmt::layer().with_target(false))
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    tracing::info!(
        "Memory Platform v{} starting up...",
        env!("CARGO_PKG_VERSION")
    );

    // Load configuration
    let config = Config::from_env().map_err(|e| anyhow::anyhow!("Config error: {}", e))?;
    let config = Arc::new(config);
    tracing::info!("Configuration loaded");

    // Connect to PostgreSQL
    let db = Arc::new(PostgresDb::connect(&config).await?);
    tracing::info!("PostgreSQL connected");

    // Run database migrations
    Migrator::run(&db.pool).await?;
    tracing::info!("Database migrations applied");

    // Connect to Redis (optional — warn on failure)
    let redis_cache = match RedisCache::connect(&config.redis_url).await {
        Ok(cache) => {
            tracing::info!("Redis connected");
            Some(Arc::new(cache))
        }
        Err(e) => {
            tracing::warn!("Redis unavailable (caching degraded): {:#}", e);
            None
        }
    };

    // Connect to Neo4j (optional — warn on failure)
    let neo4j_client = match GraphClient::connect(
        &config.neo4j_uri,
        &config.neo4j_user,
        &config.neo4j_password,
    )
    .await
    {
        Ok(client) => {
            tracing::info!("Neo4j connected");
            Some(Arc::new(client))
        }
        Err(e) => {
            tracing::warn!("Neo4j unavailable (graph features degraded): {:#}", e);
            None
        }
    };

    // Build search engine
    let search = Arc::new(SearchEngine::new(Arc::clone(&db), Arc::clone(&config)));

    // Build embedding service
    let embedding_config = EmbeddingConfig {
        model: config.embedding_model.clone(),
        nvidia_api_url: if config.nvidia_api_key.is_empty() {
            None
        } else {
            Some(config.nvidia_api_key.clone())
        },
        nvidia_api_key: if config.nvidia_api_key.is_empty() {
            None
        } else {
            Some(config.nvidia_api_key.clone())
        },
        cache_size: config.embedding_cache_size,
    };
    let embedding_service: Option<Arc<dyn memory_platform::services::embedding::EmbeddingService>> =
        match EmbeddingServiceFactory::new(embedding_config).await {
            Ok(factory) => {
                tracing::info!("Embedding service initialized ({})", config.embedding_model);
                Some(Arc::new(factory))
            }
            Err(e) => {
                tracing::warn!("Embedding service unavailable: {:#}", e);
                None
            }
        };

    // Build business-logic services
    let decay_engine = Arc::new(DecayEngine::new(Arc::clone(&config)));

    let contradiction_detector = Arc::new(ContradictionDetector::new(db.pool.clone()));

    let context_service = Arc::new(ContextService::new(db.pool.clone(), Arc::clone(&search)));

    let experience_service = Arc::new(ExperienceService::new(db.pool.clone(), Arc::clone(&search)));

    let procedure_service = Arc::new(ProcedureService::new(db.pool.clone()));

    let ingestion_service = match &embedding_service {
        Some(embedder) => Some(Arc::new(IngestionService::new(
            db.pool.clone(),
            Arc::clone(embedder),
        ))),
        None => {
            tracing::warn!("Ingestion service unavailable (no embedding backend)");
            None
        }
    };

    // Build application state
    let state = Arc::new(AppState {
        config: (*config).clone(),
        db,
        search,
        neo4j_client,
        redis_cache,
        context_service: Some(context_service),
        contradiction_detector: Some(contradiction_detector),
        decay_engine: Some(decay_engine),
        embedding_service,
        experience_service: Some(experience_service),
        ingestion_service,
        procedure_service: Some(procedure_service),
    });

    // Build router and inject state
    let app = api::router().with_state(Arc::clone(&state));

    // Bind to address
    let addr = format!("0.0.0.0:{}", state.config.api_port);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("Listening on http://{}", addr);
    tracing::info!("API routes: 23 endpoints initialized");

    axum::serve(listener, app).await?;
    Ok(())
}
