//! Experience replay service.
//!
//! Finds relevant past experiences and updates confidence scores.

use std::sync::Arc;

use anyhow::Context;
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::postgres::{CreateExperience, PostgresDb};
use crate::models::Experience;
use crate::search::SearchEngine;

/// Confidence delta applied on successful reuse.
const SUCCESS_DELTA: f64 = 0.05;
/// Confidence delta applied on failure / revert.
const FAILURE_DELTA: f64 = -0.15;
/// Minimum confidence floor.
const MIN_CONFIDENCE: f64 = 0.0;
/// Maximum confidence ceiling.
const MAX_CONFIDENCE: f64 = 1.0;
/// Default half-life in days for score decay.
const DECAY_HALF_LIFE_DAYS: f64 = 30.0;
/// Minimum decay multiplier.
const DECAY_MIN_SCORE: f64 = 0.1;

/// Service for experience retrieval, recording, and confidence management.
#[derive(Debug)]
pub struct ExperienceService {
    pool: PgPool,
    search: Arc<SearchEngine>,
}

impl ExperienceService {
    #[must_use]
    pub fn new(pool: PgPool, search: Arc<SearchEngine>) -> Self {
        Self { pool, search }
    }

    /// Find past experiences relevant to a query.
    ///
    /// Uses hybrid search on the `experiences` table, then fetches full
    /// `Experience` rows for each matching ID.
    pub async fn find_relevant(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<Experience>, anyhow::Error> {
        // Use a zero embedding for keyword-only search when no embedder is wired.
        // The SearchEngine's hybrid mode will still run BM25 which is sufficient
        // for experience retrieval by goal text.
        let dummy_embedding = vec![0.0_f32; 384];
        let results = self
            .search
            .hybrid_search(
                "experiences",
                query,
                &dummy_embedding,
                "keyword",
                limit as i64,
            )
            .await
            .context("Failed to search experiences")?;

        if results.is_empty() {
            return Ok(Vec::new());
        }

        let ids: Vec<Uuid> = results.iter().map(|r| r.id).collect();
        let experiences = self.fetch_experiences_by_ids(&ids).await?;

        Ok(experiences)
    }

    /// Record a new experience in the database.
    pub async fn record_interaction(
        &self,
        data: CreateExperience,
    ) -> Result<Experience, anyhow::Error> {
        let db = PostgresDb {
            pool: self.pool.clone(),
        };
        db.store_experience(&data, None)
            .await
            .context("Failed to store experience")
            .map_err(Into::into)
    }

    /// Update confidence based on outcome (success or failure).
    ///
    /// Applies a delta clamped to [0.0, 1.0], matching the Python
    /// `GREATEST(0.0, LEAST(1.0, COALESCE(confidence, 0.5) + delta))`.
    pub async fn update_confidence(
        &self,
        experience_id: &str,
        success: bool,
    ) -> Result<(), anyhow::Error> {
        let delta = if success {
            SUCCESS_DELTA
        } else {
            FAILURE_DELTA
        };
        let id = Uuid::parse_str(experience_id).context("Invalid experience ID format")?;

        sqlx::query(
            "UPDATE experiences \
             SET confidence = GREATEST($1, LEAST($2, COALESCE(confidence, 0.5) + $3)) \
             WHERE id = $4",
        )
        .bind(MIN_CONFIDENCE)
        .bind(MAX_CONFIDENCE)
        .bind(delta)
        .bind(id)
        .execute(&self.pool)
        .await
        .context("Failed to update experience confidence")?;

        Ok(())
    }

    /// Decay confidence scores of old experiences based on time since creation.
    ///
    /// Returns the number of rows updated.
    pub async fn decay_scores(&self) -> Result<u64, anyhow::Error> {
        let result = sqlx::query(
            "UPDATE experiences \
             SET confidence = GREATEST($1, confidence * GREATEST($2, POW(2.0, \
                 -EXTRACT(EPOCH FROM (now() - created_at)) / 86400.0 / $3))) \
             WHERE confidence IS NOT NULL \
               AND confidence > 0.0 \
               AND created_at < now() - INTERVAL '1 day'",
        )
        .bind(MIN_CONFIDENCE)
        .bind(DECAY_MIN_SCORE)
        .bind(DECAY_HALF_LIFE_DAYS)
        .execute(&self.pool)
        .await
        .context("Failed to decay experience scores")?;

        Ok(result.rows_affected())
    }

    /// Fetch full Experience rows by a list of UUIDs.
    async fn fetch_experiences_by_ids(
        &self,
        ids: &[Uuid],
    ) -> Result<Vec<Experience>, anyhow::Error> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let placeholders: Vec<String> = (1..=ids.len()).map(|i| format!("${i}")).collect();
        let sql = format!(
            "SELECT id, agent_id, session_id, goal, reasoning_summary, actions, \
                    files_changed, result, lessons_learned, confidence, \
                    duration_seconds, tags, related_project, embedding, \
                    is_procedurized, created_at \
             FROM experiences WHERE id IN ({})",
            placeholders.join(",")
        );

        let mut query_builder = sqlx::query_as::<_, Experience>(&sql);
        for id in ids {
            query_builder = query_builder.bind(*id);
        }

        query_builder
            .fetch_all(&self.pool)
            .await
            .context("Failed to fetch experiences by IDs")
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confidence_deltas_are_reasonable() {
        assert!(SUCCESS_DELTA > 0.0 && SUCCESS_DELTA < 0.5);
        assert!(FAILURE_DELTA < 0.0 && FAILURE_DELTA > -0.5);
    }

    #[test]
    fn decay_constants_are_valid() {
        assert!(DECAY_HALF_LIFE_DAYS > 0.0);
        assert!(DECAY_MIN_SCORE > 0.0 && DECAY_MIN_SCORE < 1.0);
    }

    #[test]
    fn confidence_bounds_are_correct() {
        assert!((MIN_CONFIDENCE - 0.0).abs() < f64::EPSILON);
        assert!((MAX_CONFIDENCE - 1.0).abs() < f64::EPSILON);
    }
}
