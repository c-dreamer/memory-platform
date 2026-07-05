//! Vector search wrapper — delegates to `PostgresDb::vector_search`.
//!
//! Thin orchestration layer that converts the engine's `SearchResult`
//! type from the DB layer's `db::SearchResult`.

use crate::db::postgres::PostgresDb;

use super::SearchResult;

/// Vector-only search wrapper.
///
/// Calls `PostgresDb::vector_search` and maps DB rows into the
/// orchestration-layer `SearchResult` type.
pub struct VectorSearch;

impl VectorSearch {
    /// Execute a vector similarity search against a single table.
    ///
    /// # Arguments
    ///
    /// * `db` — the database connection pool wrapper.
    /// * `table` — one of `"memories"`, `"documents"`, `"experiences"`, `"trading_results"`.
    /// * `embedding` — the query embedding vector.
    /// * `limit` — maximum number of results to return.
    /// * `threshold` — minimum cosine similarity (0.0–1.0).
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` on database failures.
    pub async fn search(
        db: &PostgresDb,
        table: &str,
        embedding: &[f32],
        limit: i64,
        threshold: f64,
    ) -> Result<Vec<SearchResult>, sqlx::Error> {
        let rows = db.vector_search(table, embedding, limit, threshold).await?;
        Ok(rows
            .into_iter()
            .map(|r| SearchResult {
                id: r.id,
                content: r.content,
                score: r.score,
                source_info: r.source_info,
                vec_rank: None,
                kw_rank: None,
                decay_factor: None,
            })
            .collect())
    }
}
