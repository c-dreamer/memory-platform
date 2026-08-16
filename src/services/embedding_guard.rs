//! Embedding dimension guard — fails closed for semantic search when the
//! configured dimension disagrees with the database schema or stored vectors.
//!
//! Postgres stores `VECTOR(2048)` columns. If the runtime is configured with a
//! different `EMBEDDING_DIM`, every vector operator would silently misbehave or
//! error mid-query. This guard surfaces that mismatch before search runs and
//! forces a keyword-only (degraded) mode instead of comparing vectors across
//! generations.

use sqlx::PgPool;

use crate::config::{
    Config, DEFAULT_EMBEDDING_DIM, DEFAULT_EMBEDDING_GENERATION, DEFAULT_EMBEDDING_MODEL,
};

/// Operational mode for embedding-backed features.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddingMode {
    /// Full semantic search is safe and enabled.
    Semantic {
        /// Expected vector dimension.
        dimension: usize,
        /// Canonical embedding model.
        model: String,
        /// Generation label of the stored vectors.
        generation: String,
    },
    /// Semantic search is disabled; keyword/BM25 only. The reason explains why.
    KeywordOnly {
        /// Human-readable reason for the degradation.
        reason: String,
    },
    /// The service is unhealthy in a way that blocks semantic search and the
    /// guard could not establish a consistent dimension.
    Degraded {
        /// Human-readable reason.
        reason: String,
    },
}

impl EmbeddingMode {
    /// True when semantic (vector) search may run.
    #[must_use]
    pub fn semantic_enabled(&self) -> bool {
        matches!(self, Self::Semantic { .. })
    }
}

/// Result of inspecting the configured dimension against the live schema and
/// the vectors already stored in the `embeddings` table.
#[derive(Debug, Clone)]
pub struct DimensionProbe {
    /// Dimension configured in the environment (or the canonical default).
    pub configured_dim: usize,
    /// Declared dimensions of every `vector` column in the public schema.
    pub schema_dims: Vec<i32>,
    /// Distinct `dimension` values recorded on stored embedding rows.
    pub stored_dims: Vec<i32>,
    /// Distinct `model` values recorded on stored embedding rows.
    pub stored_models: Vec<String>,
    /// Distinct `embedding_generation` values recorded on stored embedding rows.
    pub stored_generations: Vec<String>,
    /// Total number of stored embedding rows.
    pub stored_total: i64,
    /// Configured embedding model.
    pub model: String,
    /// Configured generation label.
    pub generation: String,
}

impl DimensionProbe {
    /// True when every schema column and stored vector agrees with the
    /// configured dimension.
    #[must_use]
    pub fn consistent(&self) -> bool {
        let dim = self.configured_dim as i32;
        self.schema_dims.iter().all(|d| *d == dim) && self.stored_dims.iter().all(|d| *d == dim)
    }

    /// Mismatching stored rows are quarantined: never compared to current
    /// vectors. This reports whether any such rows exist.
    #[must_use]
    pub fn has_legacy_rows(&self) -> bool {
        let dim = self.configured_dim as i32;
        self.stored_dims.iter().any(|d| *d != dim)
    }
}

/// Inspect the database for vector column dimensions and stored-vector
/// metadata, then classify the safe embedding mode.
///
/// This is read-only: it never mutates data. A mismatch refuses semantic
/// search rather than silently padding/truncating vectors.
pub async fn probe_dimensions(
    pool: &PgPool,
    config: &Config,
) -> Result<DimensionProbe, sqlx::Error> {
    // Vector columns in the public schema with their declared dimension.
    // format_type(atttypid, atttypmod) yields e.g. "vector(2048)"; parse the
    // integer from that rather than deriving an offset from atttypmod, whose
    // encoding varies across pgvector versions.
    let schema_dims: Vec<i32> = sqlx::query_scalar(
        "SELECT DISTINCT (regexp_match(format_type(a.atttypid, a.atttypmod), '\\((\\d+)\\)'))[1]::int AS dim \
         FROM pg_attribute a \
         JOIN pg_class c ON c.oid = a.attrelid \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'public' \
           AND a.attnum > 0 AND NOT a.attisdropped \
           AND format_type(a.atttypid, a.atttypmod) LIKE 'vector(%'",
    )
    .fetch_all(pool)
    .await?;

    let stored_dims: Vec<i32> =
        sqlx::query_scalar("SELECT DISTINCT dimension FROM embeddings WHERE dimension IS NOT NULL")
            .fetch_all(pool)
            .await?;

    let stored_models: Vec<String> =
        sqlx::query_scalar("SELECT DISTINCT model FROM embeddings WHERE model IS NOT NULL")
            .fetch_all(pool)
            .await?;

    let stored_generations: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT embedding_generation FROM embeddings \
         WHERE embedding_generation IS NOT NULL",
    )
    .fetch_all(pool)
    .await?;

    let stored_total: i64 = sqlx::query_scalar("SELECT count(*) FROM embeddings")
        .fetch_one(pool)
        .await?;

    Ok(DimensionProbe {
        configured_dim: config.embedding_dim,
        schema_dims,
        stored_dims,
        stored_models,
        stored_generations,
        stored_total,
        model: config.nvidia_embedding_model.clone(),
        generation: config.embedding_generation.clone(),
    })
}

/// Classify the safe mode from a probe. Pure function — unit-testable.
#[must_use]
pub fn classify(probe: &DimensionProbe) -> EmbeddingMode {
    let dim = probe.configured_dim;

    if probe.schema_dims.is_empty() {
        // No vector columns present (fresh DB) — semantic search has nothing
        // to query but is not dangerous; still report degraded so callers can
        // decide, since no embeddings can be stored either.
        return EmbeddingMode::Degraded {
            reason: "no vector columns found in public schema".to_string(),
        };
    }

    if probe.schema_dims.iter().any(|d| *d as usize != dim) {
        return EmbeddingMode::KeywordOnly {
            reason: format!(
                "database vector columns ({:?}) disagree with configured EMBEDDING_DIM={dim}; \
                 refusing semantic search",
                probe.schema_dims
            ),
        };
    }

    if probe.has_legacy_rows() {
        return EmbeddingMode::KeywordOnly {
            reason: format!(
                "stored embeddings span dimensions {:?} while configured EMBEDDING_DIM={dim}; \
                 legacy rows must be re-embedded before semantic search",
                probe.stored_dims
            ),
        };
    }

    // Stored rows labelled with a different generation than configured: they may
    // share the dimension but were produced by a different model family. Refuse
    // rather than compare vectors across generations.
    if !probe.stored_generations.is_empty()
        && probe
            .stored_generations
            .iter()
            .any(|g| g != &probe.generation)
    {
        return EmbeddingMode::KeywordOnly {
            reason: format!(
                "stored embeddings carry generations {:?} but configured \
                 EMBEDDING_GENERATION={}; refusing cross-generation comparison",
                probe.stored_generations, probe.generation
            ),
        };
    }

    if dim != DEFAULT_EMBEDDING_DIM {
        // Allowed (custom dimension) but must match the canonical generation.
        if probe.generation == DEFAULT_EMBEDDING_GENERATION && dim != DEFAULT_EMBEDDING_DIM {
            return EmbeddingMode::KeywordOnly {
                reason: format!(
                    "generation {} implies {} dimensions but EMBEDDING_DIM={dim}",
                    probe.generation, DEFAULT_EMBEDDING_DIM
                ),
            };
        }
        return EmbeddingMode::Semantic {
            dimension: dim,
            model: probe.model.clone(),
            generation: probe.generation.clone(),
        };
    }

    if probe.model != DEFAULT_EMBEDDING_MODEL && !probe.model.is_empty() {
        // Warn-level divergence is tolerated: dimension is the hard constraint.
        return EmbeddingMode::Semantic {
            dimension: dim,
            model: probe.model.clone(),
            generation: probe.generation.clone(),
        };
    }

    EmbeddingMode::Semantic {
        dimension: dim,
        model: probe.model.clone(),
        generation: probe.generation.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(dim: usize) -> DimensionProbe {
        DimensionProbe {
            configured_dim: dim,
            schema_dims: vec![dim as i32],
            stored_dims: vec![dim as i32],
            stored_models: vec![DEFAULT_EMBEDDING_MODEL.to_string()],
            stored_generations: vec![DEFAULT_EMBEDDING_GENERATION.to_string()],
            stored_total: 10,
            model: DEFAULT_EMBEDDING_MODEL.to_string(),
            generation: DEFAULT_EMBEDDING_GENERATION.to_string(),
        }
    }

    #[test]
    fn clean_2048_is_semantic() {
        let p = probe(2048);
        assert!(p.consistent());
        assert_eq!(
            classify(&p),
            EmbeddingMode::Semantic {
                dimension: 2048,
                model: DEFAULT_EMBEDDING_MODEL.to_string(),
                generation: DEFAULT_EMBEDDING_GENERATION.to_string(),
            }
        );
        assert!(classify(&p).semantic_enabled());
    }

    #[test]
    fn schema_mismatch_refuses_semantic() {
        let mut p = probe(2048);
        p.schema_dims = vec![384];
        assert!(!p.consistent());
        assert!(matches!(classify(&p), EmbeddingMode::KeywordOnly { .. }));
    }

    #[test]
    fn legacy_stored_rows_refuse_semantic() {
        let mut p = probe(2048);
        p.stored_dims = vec![2048, 384];
        assert!(p.has_legacy_rows());
        assert!(matches!(classify(&p), EmbeddingMode::KeywordOnly { .. }));
    }

    #[test]
    fn no_schema_is_degraded() {
        let mut p = probe(2048);
        p.schema_dims = vec![];
        assert!(matches!(classify(&p), EmbeddingMode::Degraded { .. }));
    }

    #[test]
    fn generation_dim_mismatch_refuses_semantic() {
        let mut p = probe(384);
        p.schema_dims = vec![384];
        p.stored_dims = vec![384];
        assert!(matches!(classify(&p), EmbeddingMode::KeywordOnly { .. }));
    }

    #[test]
    fn stored_generation_mismatch_refuses_semantic() {
        let mut p = probe(2048);
        p.stored_generations = vec![
            DEFAULT_EMBEDDING_GENERATION.to_string(),
            "legacy-generation".to_string(),
        ];
        assert!(matches!(classify(&p), EmbeddingMode::KeywordOnly { .. }));
    }

    #[test]
    fn empty_stored_generation_is_semantic() {
        let mut p = probe(2048);
        p.stored_generations = vec![];
        assert!(classify(&p).semantic_enabled());
    }
}
