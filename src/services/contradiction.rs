//! Contradiction detection service.
//!
//! Finds semantically similar memories with opposing signals
//! (positive vs negative sentiment, conflicting statements).

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::{Context, Result};
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::postgres::PostgresDb;
use crate::models::{Contradiction, ContradictionCandidate, Memory};

/// Negation word pairs used to detect opposing signals in memory content.
const NEGATION_PAIRS: &[(&str, &str)] = &[
    ("buy", "sell"),
    ("long", "short"),
    ("bullish", "bearish"),
    ("increase", "decrease"),
    ("up", "down"),
    ("positive", "negative"),
    ("profitable", "unprofitable"),
    ("win", "loss"),
    ("good", "bad"),
    ("should", "shouldn't"),
    ("always", "never"),
    ("yes", "no"),
    ("support", "resistance"),
    ("overbought", "oversold"),
    ("truth", "false"),
    ("correct", "incorrect"),
    ("above", "below"),
    ("high", "low"),
    ("rise", "fall"),
    ("gain", "loss"),
    ("upward", "downward"),
    ("strong", "weak"),
    ("overweight", "underweight"),
    ("exceed", "underperform"),
    ("better", "worse"),
    ("improve", "decline"),
    ("buying", "selling"),
    ("bull", "bear"),
    ("overvalued", "undervalued"),
    ("expensive", "cheap"),
];

/// Detects contradictions between semantically similar memories.
///
/// Uses vector similarity search to find memories with high cosine similarity,
/// then checks for opposing signals in their content using negation pairs.
#[derive(Debug)]
pub struct ContradictionDetector {
    db: Arc<PostgresDb>,
}

impl ContradictionDetector {
    /// Create a new contradiction detector.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            db: Arc::new(PostgresDb { pool }),
        }
    }

    /// Find contradictions for a specific memory.
    ///
    /// Searches for semantically similar memories (cosine similarity > 0.85),
    /// then checks for opposing signals in their content.
    pub async fn detect(&self, memory_id: &str) -> Result<Vec<Contradiction>> {
        let id: Uuid = memory_id
            .parse()
            .with_context(|| format!("Invalid memory ID: {memory_id}"))?;

        let memory = self
            .db
            .get_memory(id)
            .await
            .with_context(|| format!("Failed to fetch memory {memory_id}"))?
            .with_context(|| format!("Memory {memory_id} not found"))?;

        let embedding = memory
            .embedding
            .as_ref()
            .with_context(|| format!("Memory {memory_id} has no embedding"))?;

        let candidates = self.find_candidates(&memory, embedding.as_vec()).await?;

        let mut contradictions = Vec::new();
        for c in candidates {
            self.db
                .store_contradiction(
                    c.memory_id_a,
                    c.memory_id_b,
                    &c.content_a,
                    &c.content_b,
                    c.similarity,
                    &c.contradiction_type,
                )
                .await
                .with_context(|| {
                    format!(
                        "Failed to store contradiction between {} and {}",
                        c.memory_id_a, c.memory_id_b
                    )
                })?;

            // Fetch the stored contradiction to return full row data.
            let stored = self
                .fetch_contradiction(c.memory_id_a, c.memory_id_b)
                .await?;
            if let Some(contra) = stored {
                contradictions.push(contra);
            }
        }

        Ok(contradictions)
    }

    /// Scan all memories for contradictions.
    ///
    /// Iterates every memory that has an embedding, runs detection for each,
    /// and returns all found contradictions (deduplicated by ID pair).
    pub async fn scan_all(&self) -> Result<Vec<Contradiction>> {
        let memories = self
            .fetch_all_memories_with_embeddings()
            .await
            .context("Failed to fetch memories for scan")?;

        let mut all_contradictions = Vec::new();
        let mut seen_pairs: HashSet<(Uuid, Uuid)> = HashSet::new();

        for memory in &memories {
            let embedding = match &memory.embedding {
                Some(emb) => emb.as_vec(),
                None => continue,
            };

            let candidates = self.find_candidates(memory, embedding).await?;

            for c in candidates {
                let pair = normalize_pair(c.memory_id_a, c.memory_id_b);
                if !seen_pairs.insert(pair) {
                    continue;
                }

                self.db
                    .store_contradiction(
                        c.memory_id_a,
                        c.memory_id_b,
                        &c.content_a,
                        &c.content_b,
                        c.similarity,
                        &c.contradiction_type,
                    )
                    .await
                    .with_context(|| {
                        format!(
                            "Failed to store contradiction between {} and {}",
                            c.memory_id_a, c.memory_id_b
                        )
                    })?;

                if let Some(contra) = self
                    .fetch_contradiction(c.memory_id_a, c.memory_id_b)
                    .await?
                {
                    all_contradictions.push(contra);
                }
            }
        }

        Ok(all_contradictions)
    }

    /// Update confidence based on outcome (success or failure).
    ///
    /// Applies a delta clamped to [0.0, 1.0].
    pub async fn update_confidence(&self, experience_id: &str, success: bool) -> Result<()> {
        let delta = if success { 0.05 } else { -0.15 };
        let id: Uuid = experience_id
            .parse()
            .with_context(|| format!("Invalid experience ID: {experience_id}"))?;

        sqlx::query(
            "UPDATE experiences \
             SET confidence = GREATEST(0.0, LEAST(1.0, COALESCE(confidence, 0.5) + $1)) \
             WHERE id = $2",
        )
        .bind(delta)
        .bind(id)
        .execute(&self.db.pool)
        .await
        .with_context(|| format!("Failed to update confidence for {experience_id}"))?;

        Ok(())
    }

    /// Decay confidence scores of old experiences based on time since creation.
    ///
    /// Returns the number of rows updated.
    pub async fn decay_scores(&self) -> Result<u64> {
        let result = sqlx::query(
            "UPDATE experiences \
             SET confidence = GREATEST(0.0, confidence * GREATEST(0.1, POW(2.0, \
                 -EXTRACT(EPOCH FROM (now() - created_at)) / 86400.0 / 30.0))) \
             WHERE confidence IS NOT NULL \
               AND confidence > 0.0 \
               AND created_at < now() - INTERVAL '1 day'",
        )
        .execute(&self.db.pool)
        .await
        .context("Failed to decay experience scores")?;

        Ok(result.rows_affected())
    }

    // ------------------------------------------------------------------
    // Private helpers
    // ------------------------------------------------------------------

    /// Find contradiction candidates for a given memory against similar ones.
    async fn find_candidates(
        &self,
        memory: &Memory,
        embedding: &[f32],
    ) -> Result<Vec<ContradictionCandidate>> {
        let similar = self
            .db
            .vector_search("memories", embedding, 10, 0.85)
            .await
            .context("Failed to search for similar memories")?;

        let mut candidates = Vec::new();
        let new_lower = memory.content.to_lowercase();

        for mem in &similar {
            if mem.id == memory.id {
                continue;
            }
            if mem.score < 0.85 {
                continue;
            }

            let existing_lower = mem.content.to_lowercase();
            let mut signals = 0;

            for &(a, b) in NEGATION_PAIRS {
                if (existing_lower.contains(a) && new_lower.contains(b))
                    || (existing_lower.contains(b) && new_lower.contains(a))
                {
                    signals += 1;
                }
            }

            if signals >= 1 {
                candidates.push(ContradictionCandidate {
                    memory_id_a: memory.id,
                    memory_id_b: mem.id,
                    content_a: memory.content.chars().take(200).collect(),
                    content_b: mem.content.chars().take(200).collect(),
                    similarity: (mem.score * 1000.0).round() / 1000.0,
                    contradiction_type: "semantic".into(),
                });
            }
        }

        Ok(candidates)
    }

    /// Fetch all memories that have embeddings.
    async fn fetch_all_memories_with_embeddings(&self) -> Result<Vec<Memory>> {
        sqlx::query_as::<_, Memory>(
            "SELECT id, agent_id, session_id, content, content_type, embedding, \
                    importance, tags, metadata, last_accessed_at, access_count, \
                    decay_score, created_at, updated_at \
             FROM memories WHERE embedding IS NOT NULL",
        )
        .fetch_all(&self.db.pool)
        .await
        .context("Failed to fetch memories with embeddings")
        .map_err(Into::into)
    }

    /// Fetch a stored contradiction by its two memory IDs.
    async fn fetch_contradiction(&self, id_a: Uuid, id_b: Uuid) -> Result<Option<Contradiction>> {
        let (a, b) = normalize_pair(id_a, id_b);
        sqlx::query_as::<_, Contradiction>(
            "SELECT id, memory_id_a, memory_id_b, content_a, content_b, \
                    similarity, contradiction_type, detected_by, resolved, \
                    resolution_note, created_at, updated_at \
             FROM contradictions \
             WHERE memory_id_a = $1 AND memory_id_b = $2",
        )
        .bind(a)
        .bind(b)
        .fetch_optional(&self.db.pool)
        .await
        .context("Failed to fetch contradiction")
        .map_err(Into::into)
    }
}

/// Normalize a pair of UUIDs into a canonical (smaller, larger) order.
fn normalize_pair(a: Uuid, b: Uuid) -> (Uuid, Uuid) {
    if a < b {
        (a, b)
    } else {
        (b, a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_pair_orders_consistently() {
        let a = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let b = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();
        assert_eq!(normalize_pair(a, b), (a, b));
        assert_eq!(normalize_pair(b, a), (a, b));
    }

    #[test]
    fn normalize_pair_equal_ids() {
        let id = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        assert_eq!(normalize_pair(id, id), (id, id));
    }

    #[test]
    fn negation_pairs_cover_expected_opposites() {
        let key_pairs = &[
            ("buy", "sell"),
            ("long", "short"),
            ("bullish", "bearish"),
            ("profitable", "unprofitable"),
            ("good", "bad"),
            ("always", "never"),
            ("support", "resistance"),
            ("above", "below"),
            ("high", "low"),
            ("strong", "weak"),
            ("better", "worse"),
            ("bull", "bear"),
            ("expensive", "cheap"),
        ];
        for (a, b) in key_pairs {
            assert!(NEGATION_PAIRS.contains(&(a, b)) || NEGATION_PAIRS.contains(&(b, a)));
        }
    }
}
