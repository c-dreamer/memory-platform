//! Memory Platform — main binary entry point.
//!
//! Starts the Axum HTTP server on the configured port.
//! Initializes all services, database connections, and migration runner.

use anyhow::Context;
use std::sync::Arc;
use std::time::{Duration, Instant};

use memory_platform::api;
use memory_platform::config::Config;
use memory_platform::db::neo4j::GraphClient;
use memory_platform::db::postgres::PostgresDb;
use memory_platform::db::redis::RedisCache;
use memory_platform::migrations::Migrator;
use memory_platform::queue::PendingWriteQueue;
use memory_platform::search::SearchEngine;
use memory_platform::services::contradiction::ContradictionDetector;
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

    // Connect to PostgreSQL. It is the local write cache, not an optional
    // dependency (unlike Redis/Neo4j below): retry for a bounded window to
    // ride out a startup race (e.g. a scheduled task firing before the DB
    // service is up), then exit non-zero so a supervisor restarts us rather
    // than serving traffic against a pool that can never succeed a query.
    let db = connect_with_retry(&config)
        .await
        .context("PostgreSQL is required and never became reachable")?;
    tracing::info!("PostgreSQL connected");
    let db = Arc::new(db);
    Migrator::run(&db.pool).await?;
    tracing::info!("Database migrations applied");

    // Local durability queue ahead of Postgres for POST /events. Required,
    // not optional like Redis/Neo4j below: it exists specifically so a
    // momentary Postgres outage never silently loses an event, so a broken
    // queue is treated the same as a broken primary database — fail fast.
    let pending_writes = Arc::new(
        PendingWriteQueue::open(&PendingWriteQueue::default_path()?)
            .await
            .context("pending-writes queue is required and never became available")?,
    );
    tracing::info!("Pending-writes queue ready");

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
        nvidia_api_url: if config.nvidia_api_url.is_empty() {
            None
        } else {
            Some(config.nvidia_api_url.clone())
        },
        nvidia_api_key: if config.nvidia_api_key.is_empty() {
            None
        } else {
            Some(config.nvidia_api_key.clone())
        },
        nvidia_embedding_model: config.nvidia_embedding_model.clone(),
        llama_cpp_url: config.llama_cpp_url.clone(),
        llama_cpp_model_name: config.llama_cpp_model_name.clone(),
        expected_dimension: config.embedding_dim,
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
    let contradiction_detector = Arc::new(ContradictionDetector::new(db.pool.clone()));

    let experience_service = Arc::new(ExperienceService::new(
        db.pool.clone(),
        Arc::clone(&search),
        embedding_service.clone(),
    ));

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
        contradiction_detector: Some(contradiction_detector),
        embedding_service,
        experience_service: Some(experience_service),
        ingestion_service,
        procedure_service: Some(procedure_service),
        pending_writes: Arc::clone(&pending_writes),
    });

    // Background drain: retry anything left in the queue from a past
    // Postgres outage. Stops at the first failure in a batch rather than
    // hammering a database that is still down; the next tick tries again.
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            loop {
                interval.tick().await;
                if let Err(e) = api::handlers::events::drain_pending(&state).await {
                    tracing::warn!("pending-writes drain failed: {e:#}");
                }
            }
        });
    }

    // Build router and inject state
    let app = api::router().with_state(Arc::clone(&state));

    // Bind to address
    let addr = format!("{}:{}", state.config.api_bind, state.config.api_port);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("Listening on http://{}", addr);
    tracing::info!("API routes: 23 endpoints initialized");

    axum::serve(listener, app).await?;
    Ok(())
}

/// Retry the PostgreSQL connection with a fixed backoff for up to
/// `MAX_WAIT`, then give up. Bounded so a real outage still exits the
/// process (for a supervisor to restart) instead of retrying forever.
async fn connect_with_retry(config: &Config) -> anyhow::Result<PostgresDb> {
    const RETRY_INTERVAL: Duration = Duration::from_secs(2);
    const MAX_WAIT: Duration = Duration::from_secs(120);

    let deadline = Instant::now() + MAX_WAIT;
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        match PostgresDb::connect(config).await {
            Ok(db) => return Ok(db),
            Err(e) if Instant::now() < deadline => {
                tracing::warn!(
                    "PostgreSQL connect attempt {attempt} failed, retrying in {RETRY_INTERVAL:?}: {e}"
                );
                tokio::time::sleep(RETRY_INTERVAL).await;
            }
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "PostgreSQL still unreachable after {attempt} attempts over {MAX_WAIT:?}: {e}"
                ))
            }
        }
    }
}
