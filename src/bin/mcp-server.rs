//! MCP stdio server — standalone binary entry point.
//!
//! Implements the Model Context Protocol over stdin/stdout.

use std::sync::Arc;
use std::time::Duration;

use memory_platform::config::Config;
use memory_platform::db::postgres::PostgresDb;
use memory_platform::mcp;
use memory_platform::queue::PendingWriteQueue;
use memory_platform::search::SearchEngine;
use memory_platform::services::contradiction::ContradictionDetector;
use memory_platform::services::embedding::{
    EmbeddingConfig, EmbeddingService, EmbeddingServiceFactory,
};
use memory_platform::services::experience::ExperienceService;
use memory_platform::services::procedure::ProcedureService;

/// Startup must finish well inside the ~30 s MCP clients allow for
/// `initialize`, so neither probe may block it for long. A probe that runs
/// out of time only means degraded (DB) or keyword-only (embedding) mode.
const STARTUP_DB_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const EMBEDDING_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

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

    // The pool is lazy: no connection is opened here, so an unreachable
    // database cannot stall the MCP handshake (the client gives up after
    // ~30 s). Every later query re-dials, so the server recovers on its own
    // once PostgreSQL is back; until then DB-backed tools return a clear
    // "database unavailable" error. Schema changes stay an explicit
    // deployment operation — a short-lived MCP session never runs DDL.
    let db = Arc::new(PostgresDb::connect_lazy(config.as_ref())?);

    // Embedding service is optional — tools fall back to keyword search if
    // unavailable. An empty key cannot authenticate, so treat it as absent
    // instead of spending the probe's retries on it.
    let embedding_config = EmbeddingConfig {
        model: config.embedding_model.clone(),
        nvidia_api_url: Some(config.nvidia_api_url.clone()),
        nvidia_api_key: Some(config.nvidia_api_key.clone()).filter(|key| !key.is_empty()),
        nvidia_embedding_model: config.nvidia_embedding_model.clone(),
        llama_cpp_url: config.llama_cpp_url.clone(),
        llama_cpp_model_name: config.llama_cpp_model_name.clone(),
        expected_dimension: config.embedding_dim,
        cache_size: config.embedding_cache_size,
    };
    let embedding_probe = async {
        let svc = EmbeddingServiceFactory::new(embedding_config).await?;
        let probe = tokio::time::timeout(
            EMBEDDING_PROBE_TIMEOUT,
            svc.embed("__memory_platform_embedding_probe__"),
        )
        .await
        .map_err(|_| anyhow::anyhow!("no answer within {EMBEDDING_PROBE_TIMEOUT:?}"))??;
        anyhow::Ok((svc, probe))
    };
    // Both startup probes are bounded and run concurrently, so the worst case
    // before the server starts listening is the longer of the two timeouts.
    let (active_dimensions, embedding_probe) = tokio::join!(
        tokio::time::timeout(STARTUP_DB_PROBE_TIMEOUT, db.embedding_dimensions()),
        embedding_probe
    );
    let active_dimensions = match active_dimensions {
        Ok(Ok(dimensions)) => {
            tracing::info!(
                "Connected to PostgreSQL; migration state will be reported by memory_health"
            );
            dimensions
        }
        Ok(Err(e)) => {
            tracing::warn!("PostgreSQL unavailable, starting MCP in degraded mode: {e}");
            Vec::new()
        }
        Err(_) => {
            tracing::warn!("PostgreSQL did not answer within {STARTUP_DB_PROBE_TIMEOUT:?}, starting MCP in degraded mode");
            Vec::new()
        }
    };
    if active_dimensions.len() > 1 {
        tracing::error!(?active_dimensions, "Mixed active embedding dimensions; vector search will remain disabled until controlled re-embedding");
    }

    let embedding_service: Option<Arc<dyn EmbeddingService>> = match embedding_probe {
        Ok((svc, probe))
            if active_dimensions.len() <= 1
                && (config.embedding_dim == 0 || probe.as_vec().len() == config.embedding_dim) =>
        {
            tracing::info!(model = %config.embedding_model, dimension = probe.as_vec().len(), "Embedding probe passed");
            Some(Arc::new(svc))
        }
        Ok((_, probe)) => {
            tracing::error!(
                configured = config.embedding_dim,
                actual = probe.as_vec().len(),
                "Embedding dimension mismatch; keyword-only mode"
            );
            None
        }
        Err(e) => {
            tracing::warn!("Embedding service unavailable (keyword-only fallback): {e:#}");
            None
        }
    };

    // Clone pool for services that need PgPool directly
    let pool = db.pool.clone();

    // Initialize services
    let search = Arc::new(SearchEngine::new(Arc::clone(&db), Arc::clone(&config)));

    let contradiction_detector = Arc::new(ContradictionDetector::new(pool.clone()));
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
        contradiction_detector: Some(contradiction_detector),
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
