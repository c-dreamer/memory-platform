//! BM25 keyword search wrapper — delegates to `PostgresDb::bm25_search`.
//!
//! Thin orchestration layer that converts the engine's `SearchResult`
//! type from the DB layer's `db::SearchResult`.

use crate::db::postgres::PostgresDb;

use super::SearchResult;

/// Keyword-only (BM25) search wrapper.
///
/// Calls `PostgresDb::bm25_search` and maps DB rows into the
/// orchestration-layer `SearchResult` type.
pub struct Bm25Search;

impl Bm25Search {
    /// Execute a BM25 full-text search against a single table.
    ///
    /// Uses PostgreSQL tsvector/tsquery with `ts_rank`, falling back
    /// to pg_trgm similarity when the tsvector index is unavailable.
    ///
    /// # Arguments
    ///
    /// * `db` — the database connection pool wrapper.
    /// * `table` — one of `"memories"`, `"documents"`, `"experiences"`, `"trading_results"`.
    /// * `query` — the raw text query string.
    /// * `limit` — maximum number of results to return.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` on database failures.
    pub async fn search(
        db: &PostgresDb,
        table: &str,
        query: &str,
        limit: i64,
    ) -> Result<Vec<SearchResult>, sqlx::Error> {
        let rows = db.bm25_search(table, query, limit).await?;
        Ok(rows
            .into_iter()
            .map(|r| SearchResult {
                id: r.id,
                content: r.content.unwrap_or_default(),
                score: r.score,
                source_info: r.source_info,
                vec_rank: None,
                kw_rank: None,
                decay_factor: None,
            })
            .collect())
    }
}
