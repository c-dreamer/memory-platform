//! Memory Platform — Rust rewrite of the Python memory-manager.
//!
//! Provides an Axum HTTP server, MCP stdio server, PostgreSQL/Redis/Neo4j
//! database layer, hybrid search (RRF + BM25 + vector), memory decay,
//! contradiction detection, embedding service, and Obsidian vault ingestion.

pub mod api;
pub mod config;
pub mod db;
pub mod mcp;
pub mod migrations;
pub mod models;
pub mod search;
pub mod services;

use std::sync::Arc;

pub use config::Config;

use db::neo4j::GraphClient;
use db::postgres::PostgresDb;
use db::redis::RedisCache;
use search::SearchEngine;
use services::context::ContextService;
use services::contradiction::ContradictionDetector;
use services::decay::DecayEngine;
use services::embedding::EmbeddingService;
use services::experience::ExperienceService;
use services::ingestion::IngestionService;
use services::procedure::ProcedureService;

/// Shared application state passed to all handlers.
#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub db: Arc<PostgresDb>,
    pub search: Arc<SearchEngine>,
    pub neo4j_client: Option<Arc<GraphClient>>,
    pub redis_cache: Option<Arc<RedisCache>>,
    pub context_service: Option<Arc<ContextService>>,
    pub contradiction_detector: Option<Arc<ContradictionDetector>>,
    pub decay_engine: Option<Arc<DecayEngine>>,
    pub embedding_service: Option<Arc<dyn EmbeddingService>>,
    pub experience_service: Option<Arc<ExperienceService>>,
    pub ingestion_service: Option<Arc<IngestionService>>,
    pub procedure_service: Option<Arc<ProcedureService>>,
}
