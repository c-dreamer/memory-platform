//! Memory Platform — Rust rewrite of the Python memory-manager.
//!
//! Provides an Axum HTTP server, MCP stdio server, PostgreSQL/Redis/Neo4j
//! database layer, hybrid search (RRF + BM25 + vector), memory decay,
//! contradiction detection, embedding service, and Obsidian vault ingestion.

pub mod api;
pub mod config;
pub mod db;
pub mod ingest;
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

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("config", &self.config)
            .field("db", &self.db)
            .field("search", &self.search)
            .field("neo4j_client", &self.neo4j_client.as_ref().map(|_| "(GraphClient)"))
            .field("redis_cache", &self.redis_cache.as_ref().map(|_| "(RedisCache)"))
            .field("context_service", &self.context_service)
            .field("contradiction_detector", &self.contradiction_detector)
            .field("decay_engine", &self.decay_engine)
            .field("embedding_service", &self.embedding_service.as_ref().map(|_| "(EmbeddingService)"))
            .field("experience_service", &self.experience_service)
            .field("ingestion_service", &self.ingestion_service.as_ref().map(|_| "(IngestionService)"))
            .field("procedure_service", &self.procedure_service)
            .finish()
    }
}
