//! API module — HTTP handlers and router.
//!
//! Contains all 23 routes matching the Python FastAPI server.

pub mod auth;
pub mod dto;
pub mod handlers;

use axum::Router;
use std::sync::Arc;
use tower_http::cors::CorsLayer;

use crate::AppState;
#[cfg(feature = "transport-http")]
use crate::mcp::transport::http_json_rpc_handler;

/// Build the API router with all routes.
#[must_use]
pub fn router() -> Router<Arc<AppState>> {
    let router = Router::new()
        .route("/", axum::routing::get(handlers::root::root))
        .route("/health", axum::routing::get(handlers::root::health_check))
        // Agents
        .route(
            "/agents/register",
            axum::routing::post(handlers::agents::register_agent),
        )
        .route(
            "/agents/{agent_id}",
            axum::routing::get(handlers::agents::get_agent),
        )
        // Sessions
        .route(
            "/sessions",
            axum::routing::post(handlers::sessions::create_session),
        )
        .route(
            "/sessions/{session_id}",
            axum::routing::get(handlers::sessions::get_session),
        )
        .route(
            "/sessions/{session_id}/summarize",
            axum::routing::post(handlers::sessions::summarize_session),
        )
        // Memories
        .route(
            "/memories",
            axum::routing::post(handlers::memories::store_memory),
        )
        .route(
            "/memories/{memory_id}",
            axum::routing::get(handlers::memories::get_memory),
        )
        // Search
        .route("/search", axum::routing::get(handlers::search::search))
        .route(
            "/search/similar",
            axum::routing::get(handlers::search::search_similar),
        )
        // Context
        .route(
            "/context",
            axum::routing::get(handlers::context::get_context),
        )
        // Procedures
        .route(
            "/procedures",
            axum::routing::get(handlers::procedures::list_procedures)
                .post(handlers::procedures::create_procedure),
        )
        .route(
            "/procedures/{procedure_id}",
            axum::routing::get(handlers::procedures::get_procedure),
        )
        .route(
            "/procedures/execute",
            axum::routing::post(handlers::procedures::execute_procedure),
        )
        .route(
            "/procedures/{procedure_id}/execute",
            axum::routing::post(handlers::procedures::execute_procedure_by_id),
        )
        .route(
            "/procedures/detect",
            axum::routing::post(handlers::procedures::detect_procedure_candidates),
        )
        // Ingestion
        .route(
            "/ingest",
            axum::routing::post(handlers::ingestion::trigger_ingest),
        )
        // Events
        .route(
            "/events",
            axum::routing::post(handlers::events::ingest_event),
        )
        // Experiences
        .route(
            "/experiences",
            axum::routing::get(handlers::experiences::list_experiences),
        )
        .route(
            "/experiences/relevant",
            axum::routing::get(handlers::experiences::find_relevant_experiences),
        )
        .route(
            "/experiences/{experience_id}/confidence",
            axum::routing::patch(handlers::experiences::update_experience_confidence),
        );

    // MCP Streamable HTTP endpoint (behind transport-http feature).
    #[cfg(feature = "transport-http")]
    let router = router.route("/mcp", axum::routing::post(http_json_rpc_handler));

    router.layer(CorsLayer::permissive())
}
