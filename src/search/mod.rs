//! Search module — RRF hybrid search engine orchestration layer.
//!
//! Wraps the low-level `PostgresDb` search methods (`vector_search`,
//! `bm25_search`) with a higher-level `SearchEngine` that dispatches
//! across three modes:
//!
//! * `"vector"` — vector similarity only (pgvector cosine distance).
//! * `"keyword"` — BM25 full-text only (tsvector/tsquery + trigram fallback).
//! * `"hybrid"` — both vector + keyword, fused via Reciprocal Rank Fusion.
//!
//! The engine supports all four searchable tables: `memories`, `documents`,
//! `experiences`, and `trading_results`.

pub mod bm25;
pub mod rrf;
pub mod rsf;
pub mod vector;

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::Config;
use crate::db::postgres::PostgresDb;

use self::bm25::Bm25Search;
use self::rrf::RrfFusion;
use self::rsf::RsfFusion;
use self::vector::VectorSearch;

// ---------------------------------------------------------------------------
// SearchResult — the orchestration-layer result type
// ---------------------------------------------------------------------------

/// A single search result produced by the orchestration layer.
///
/// Carries the result ID, content text, relevance score, source metadata,
/// and optional rank / decay information from the fusion process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// The result's UUID.
    pub id: Uuid,
    /// The text content of the result.
    pub content: String,
    /// Relevance score (cosine similarity, BM25 rank, or fused RRF score).
    pub score: f64,
    /// Source metadata (tags, path, strategy — varies by table).
    pub source_info: String,
    /// Rank in the vector result list (1-based), if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vec_rank: Option<i32>,
    /// Rank in the keyword result list (1-based), if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kw_rank: Option<i32>,
    /// Memory decay factor (only populated for "memories" table when decay is enabled).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decay_factor: Option<f64>,
}

// ---------------------------------------------------------------------------
// SearchEngine
// ---------------------------------------------------------------------------

/// High-level search orchestrator.
///
/// Wraps a `PostgresDb` connection pool and the application `Config`
/// (for RRF parameters and decay settings). Provides a single
/// `hybrid_search` entry-point that dispatches to vector-only,
/// keyword-only, or full hybrid (RRF-fused) search depending on the
/// requested mode.
#[derive(Debug)]
pub struct SearchEngine {
    db: Arc<PostgresDb>,
    config: Arc<Config>,
}

impl SearchEngine {
    /// Create a new search engine.
    ///
    /// Both `db` and `config` are wrapped in `Arc` so the engine can
    /// be shared across threads (e.g. in an Axum application state).
    #[must_use]
    pub fn new(db: Arc<PostgresDb>, config: Arc<Config>) -> Self {
        Self { db, config }
    }

    /// Create an empty search engine for testing (no real backend).
    #[must_use]
    pub fn new_empty() -> Self {
        Self {
            db: Arc::new(PostgresDb::new_empty()),
            config: Arc::new(Config::default()),
        }
    }

    /// Execute a search against a single table with the given mode.
    ///
    /// # Arguments
    ///
    /// * `table` — one of `"memories"`, `"documents"`, `"experiences"`,
    ///   `"trading_results"`.
    /// * `query` — the raw text query (used for BM25 and for embedding generation).
    /// * `embedding` — the query embedding vector (pre-computed by the caller).
    /// * `mode` — `"vector"`, `"keyword"`, or `"hybrid"`.
    /// * `limit` — maximum number of results to return.
    ///
    /// # Mode behaviour
    ///
    /// | Mode       | Vector search | BM25 search | RRF fusion |
    /// |------------|---------------|-------------|------------|
    /// | `"vector"` | yes           | no          | no         |
    /// | `"keyword"`| no            | yes         | no         |
    /// | `"hybrid"` | yes           | yes         | yes        |
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` on database failures.
    pub async fn hybrid_search(
        &self,
        table: &str,
        query: &str,
        embedding: &[f32],
        mode: &str,
        limit: i64,
    ) -> Result<Vec<SearchResult>, sqlx::Error> {
        match mode {
            "vector" => VectorSearch::search(&self.db, table, embedding, limit, 0.0).await,
            "keyword" => Bm25Search::search(&self.db, table, query, limit).await,
            "rsf" => {
                let fetch_limit = limit * 2;

                let (vec_results, kw_results) = tokio::try_join!(
                    VectorSearch::search(&self.db, table, embedding, fetch_limit, 0.0),
                    Bm25Search::search(&self.db, table, query, fetch_limit),
                )?;

                let mut fused = RsfFusion::fuse(
                    vec_results,
                    kw_results,
                    self.config.rrf_vector_weight,
                    self.config.rrf_keyword_weight,
                    0.1,
                );

                if self.config.decay_enabled
                    && self.config.decay_apply_to_search
                    && table == "memories"
                {
                    self.apply_decay(&mut fused).await?;
                }

                fused.truncate(limit as usize);
                Ok(fused)
            }
            "hybrid" | _ => {
                // Fetch more candidates than needed so RRF has a richer pool.
                let fetch_limit = limit * 2;

                // Run vector and BM25 in parallel.
                let (vec_results, kw_results) = tokio::try_join!(
                    VectorSearch::search(&self.db, table, embedding, fetch_limit, 0.0),
                    Bm25Search::search(&self.db, table, query, fetch_limit),
                )?;

                let mut fused = RrfFusion::fuse(
                    vec_results,
                    kw_results,
                    self.config.rrf_k,
                    self.config.rrf_vector_weight,
                    self.config.rrf_keyword_weight,
                );

                // Apply memory decay when enabled and table is "memories".
                if self.config.decay_enabled
                    && self.config.decay_apply_to_search
                    && table == "memories"
                {
                    self.apply_decay(&mut fused).await?;
                }

                fused.truncate(limit as usize);
                Ok(fused)
            }
        }
    }

    /// Apply Ebbinghaus-inspired memory decay to fused results.
    ///
    /// Queries `last_accessed_at` / `created_at` for each memory ID,
    /// computes `decay = max(min_score, 2^(-days_since / half_life))`,
    /// and multiplies each result's score by its decay factor.
    async fn apply_decay(&self, results: &mut Vec<SearchResult>) -> Result<(), sqlx::Error> {
        use sqlx::Row;

        let ids: Vec<Uuid> = results.iter().map(|r| r.id).collect();
        let half_life = self.config.decay_half_life_days;
        let min_score = self.config.decay_min_score;

        for chunk in ids.chunks(50) {
            let placeholders: Vec<String> = (1..=chunk.len()).map(|i| format!("${i}")).collect();
            let sql = format!(
                "SELECT id, (EXTRACT(EPOCH FROM (now() - COALESCE(last_accessed_at, created_at))) / 86400.0)::FLOAT8 AS days_since \
                 FROM memories WHERE id IN ({})",
                placeholders.join(",")
            );

            let mut query_builder = sqlx::query(&sql);
            for id in chunk {
                query_builder = query_builder.bind(*id);
            }
            let rows = query_builder.fetch_all(&self.db.pool).await?;

            for row in &rows {
                let id: Uuid = row.get("id");
                let days_since: f64 = row.get("days_since");
                let decay = f64::max(min_score, 2_f64.powf(-days_since / half_life));

                if let Some(r) = results.iter_mut().find(|r| r.id == id) {
                    r.score *= decay;
                    r.decay_factor = Some(decay);
                }
            }
        }

        // Re-sort after decay application.
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_result_serde_roundtrip() {
        let sr = SearchResult {
            id: Uuid::new_v4(),
            content: "Rust is memory-safe".into(),
            score: 0.95,
            source_info: "rust,memory".into(),
            vec_rank: Some(1),
            kw_rank: Some(3),
            decay_factor: Some(0.5),
        };
        let json = serde_json::to_string(&sr).unwrap();
        let decoded: SearchResult = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.content, "Rust is memory-safe");
        assert_eq!(decoded.score, 0.95);
        assert_eq!(decoded.vec_rank, Some(1));
        assert_eq!(decoded.kw_rank, Some(3));
        assert_eq!(decoded.decay_factor, Some(0.5));
    }

    #[test]
    fn search_result_optional_fields_serialize_as_absent_when_none() {
        let sr = SearchResult {
            id: Uuid::new_v4(),
            content: "Minimal".into(),
            score: 0.5,
            source_info: "".into(),
            vec_rank: None,
            kw_rank: None,
            decay_factor: None,
        };
        let json = serde_json::to_string(&sr).unwrap();
        assert!(!json.contains("vec_rank"));
        assert!(!json.contains("kw_rank"));
        assert!(!json.contains("decay_factor"));
    }

    #[tokio::test]
    async fn search_engine_constructs_with_arcs() {
        let config = Arc::new(Config::default());
        let _engine = SearchEngine {
            db: Arc::new(PostgresDb::new_empty()),
            config,
        };
    }
}
