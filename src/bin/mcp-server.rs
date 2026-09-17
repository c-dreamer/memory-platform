//! MCP stdio server — standalone binary entry point.
//!
//! Implements the Model Context Protocol over stdin/stdout.

use std::sync::Arc;

use memory_platform::config::Config;
use memory_platform::db::postgres::PostgresDb;
use memory_platform::mcp;
use memory_platform::queue::PendingWriteQueue;
use memory_platform::search::SearchEngine;
use memory_platform::services::context::ContextService;
use memory_platform::services::contradiction::ContradictionDetector;
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
    // handshake and use any tools that do not need live storage. A short
    // bounded retry rides out a transient blip (e.g. the DB service still
    // starting) without stalling the MCP handshake the way main.rs's much
    // longer supervised-daemon retry would.
    let db = match connect_with_short_retry(config.as_ref()).await {
        Ok(db) => {
            tracing::info!("Connected to PostgreSQL");
            let db = Arc::new(db);
            // Schema changes are an explicit deployment operation.  Starting a
            // client must remain read-only so a short-lived MCP session cannot
            // unexpectedly run DDL against the authoritative database.
            tracing::info!(
                "Connected to PostgreSQL; migration state will be reported by memory_health"
            );
            db
        }
        Err(e) => {
            tracing::warn!("PostgreSQL unavailable, starting MCP in degraded mode: {e}");
            Arc::new(PostgresDb::new_empty())
        }
    };

    let active_dimensions = db.embedding_dimensions().await.unwrap_or_default();
    if active_dimensions.len() > 1 {
        tracing::error!(?active_dimensions, "Mixed active embedding dimensions; vector search will remain disabled until controlled re-embedding");
    }

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
            llama_cpp_url: config.llama_cpp_url.clone(),
            llama_cpp_model_name: config.llama_cpp_model_name.clone(),
            expected_dimension: config.embedding_dim,
            cache_size: config.embedding_cache_size,
        };
        match EmbeddingServiceFactory::new(embedding_config).await {
            Ok(svc) => match svc.embed("__memory_platform_embedding_probe__").await {
                Ok(probe)
                    if active_dimensions.len() <= 1
                        && (config.embedding_dim == 0
                            || probe.as_vec().len() == config.embedding_dim) =>
                {
                    tracing::info!(model = %config.embedding_model, dimension = probe.as_vec().len(), "Embedding probe passed");
                    Some(Arc::new(svc))
                }
                Ok(probe) => {
                    tracing::error!(
                        configured = config.embedding_dim,
                        actual = probe.as_vec().len(),
                        "Embedding dimension mismatch; keyword-only mode"
                    );
                    None
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Embedding probe failed; keyword-only mode");
                    None
                }
            },
            Err(e) => {
                tracing::warn!("Embedding service unavailable (keyword-only fallback): {e}");
                None
            }
        }
    };

    let context_service = Arc::new(ContextService::new(pool.clone(), Arc::clone(&search)));
    let contradiction_detector = Arc::new(ContradictionDetector::new(pool.clone()));
    let decay_engine = Arc::new(DecayEngine::new(Arc::clone(&config)));
    let experience_service = Arc::new(ExperienceService::new(
        pool.clone(),
        Arc::clone(&search),
        embedding_service.clone(),
    ));
    let procedure_service = Arc::new(ProcedureService::new(pool.clone()));

    // This binary tolerates a degraded start (see the PostgreSQL match
    // above), so the pending-writes queue gets the same treatment: try the
    // real on-disk queue, fall back to an unopened in-memory placeholder
    // rather than failing the whole MCP session over it.
    let pending_writes = Arc::new(match PendingWriteQueue::default_path() {
        Ok(path) => match PendingWriteQueue::open(&path).await {
            Ok(q) => q,
            Err(e) => {
                tracing::warn!("pending-writes queue unavailable: {e:#}");
                PendingWriteQueue::new_empty()
            }
        },
        Err(e) => {
            tracing::warn!("pending-writes queue path unavailable: {e:#}");
            PendingWriteQueue::new_empty()
        }
    });

    // Build AppState
    let state = Arc::new(memory_platform::AppState {
        config: (*config).clone(),
        db: Arc::clone(&db),
        search,
        neo4j_client: None,
        redis_cache: None,
        context_service: Some(context_service),
        contradiction_detector: Some(contradiction_detector),
        decay_engine: Some(decay_engine),
        embedding_service,
        experience_service: Some(experience_service),
        ingestion_service: None,
        procedure_service: Some(procedure_service),
        pending_writes,
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

/// Retry the PostgreSQL connection a few times with a short fixed backoff,
/// then give up and let the caller fall back to degraded mode. Unlike
/// `main.rs`'s `connect_with_retry` (a supervised daemon that can afford to
/// block for minutes and exit non-zero for a restart), this runs once per
/// stdio MCP invocation — the client is waiting on the handshake, so the
/// window stays short and failure degrades gracefully instead of exiting.
async fn connect_with_short_retry(config: &Config) -> anyhow::Result<PostgresDb> {
    const RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
    const MAX_ATTEMPTS: u32 = 3;

    let mut last_err = None;
    for attempt in 1..=MAX_ATTEMPTS {
        match PostgresDb::connect(config).await {
            Ok(db) => return Ok(db),
            Err(e) => {
                if attempt < MAX_ATTEMPTS {
                    tracing::warn!(
                        "PostgreSQL connect attempt {attempt}/{MAX_ATTEMPTS} failed, retrying in {RETRY_INTERVAL:?}: {e}"
                    );
                    tokio::time::sleep(RETRY_INTERVAL).await;
                }
                last_err = Some(e);
            }
        }
    }
    Err(last_err.expect("loop runs at least once").into())
}
