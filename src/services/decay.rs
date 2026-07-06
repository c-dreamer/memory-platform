//! Memory decay engine with coherence-weighted scoring.
//!
//! Implements a weighted formula combining recency (Ebbinghaus), usage
//! frequency, and semantic coherence (cosine similarity):
//!
//! ```text
//! score = α · recency_score + β · frequency_score + γ · coherence_score
//!
//! recency_score   = max(min_score, 2^(-days_since / half_life))
//! frequency_score = min(access_count / threshold, 1.0)
//! coherence_score = semantic similarity (passed in from caller)
//! ```
//!
//! Weights α, β, γ and frequency threshold are configurable via `Config`.

use std::sync::Arc;

/// Memory decay engine with coherence-weighted scoring.
///
/// Combines recency (Ebbinghaus), frequency, and semantic coherence into
/// a single relevance score. Uses the existing `Config` for all parameters.
/// When `enabled` is false, `score()` returns 1.0 (no-op).
#[derive(Debug, Clone)]
pub struct DecayEngine {
    half_life_days: f64,
    min_score: f64,
    enabled: bool,
    recency_weight: f64,
    frequency_weight: f64,
    semantic_weight: f64,
    frequency_threshold: f64,
}

impl DecayEngine {
    /// Create a new decay engine from configuration.
    #[must_use]
    pub fn new(config: Arc<crate::config::Config>) -> Self {
        Self {
            half_life_days: config.decay_half_life_days,
            min_score: config.decay_min_score,
            enabled: config.decay_enabled,
            recency_weight: config.coherence_weight_recency,
            frequency_weight: config.coherence_weight_frequency,
            semantic_weight: config.coherence_weight_semantic,
            frequency_threshold: config.coherence_frequency_threshold,
        }
    }

    /// Compute the recency score (Ebbinghaus decay).
    ///
    /// Formula: `max(min_score, 2^(-days_since / half_life))`
    /// Returns the decay factor, clamped to `min_score`.
    #[must_use]
    pub fn compute_recency(&self, days_since_access: f64) -> f64 {
        if !self.enabled {
            return 1.0;
        }
        let factor = 2.0f64.powf(-days_since_access / self.half_life_days);
        factor.max(self.min_score)
    }

    /// Compute the frequency score.
    ///
    /// Formula: `min(access_count / threshold, 1.0)`
    /// Normalised access count, capped at 1.0.
    #[must_use]
    pub fn compute_frequency(&self, access_count: f64) -> f64 {
        if !self.enabled {
            return 1.0;
        }
        (access_count / self.frequency_threshold).min(1.0)
    }

    /// Compute the combined coherence score.
    ///
    /// Formula: `α · recency + β · frequency + γ · coherence`
    ///
    /// When disabled, always returns 1.0.
    #[must_use]
    pub fn score(&self, days_since_access: f64, access_count: f64, coherence: f64) -> f64 {
        if !self.enabled {
            return 1.0;
        }
        let recency = self.compute_recency(days_since_access);
        let frequency = self.compute_frequency(access_count);
        recency * self.recency_weight
            + frequency * self.frequency_weight
            + coherence * self.semantic_weight
    }

    /// Apply the combined coherence score to an existing relevance score.
    ///
    /// Returns: `relevance_score * score(days_since, access_count, coherence)`
    /// When disabled, returns `relevance_score` unchanged.
    #[must_use]
    pub fn apply(&self, relevance_score: f64, days_since: f64, access_count: f64, coherence: f64) -> f64 {
        if !self.enabled {
            return relevance_score;
        }
        relevance_score * self.score(days_since, access_count, coherence)
    }

    // Legacy API compatibility — kept for callers not yet migrated.
    /// Compute the legacy decay factor.
    #[must_use]
    pub fn compute_decay(&self, days_since_access: f64) -> f64 {
        self.compute_recency(days_since_access)
    }

    /// Apply legacy decay to a score.
    #[must_use]
    pub fn apply_decay(&self, rrf_score: f64, days_since: f64) -> f64 {
        rrf_score * self.compute_recency(days_since)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::sync::Arc;

    /// Helper to create a test config with decay enabled.
    fn test_config() -> Arc<Config> {
        let mut config = Config::default();
        config.decay_enabled = true;
        config.decay_half_life_days = 90.0;
        config.decay_min_score = 0.1;
        Arc::new(config)
    }

    /// Helper to create a test config with decay disabled.
    fn disabled_config() -> Arc<Config> {
        let mut config = Config::default();
        config.decay_enabled = false;
        Arc::new(config)
    }

    #[test]
    fn test_decay_engine_new() {
        let config = test_config();
        let engine = DecayEngine::new(config.clone());

        assert_eq!(engine.half_life_days, config.decay_half_life_days);
        assert_eq!(engine.min_score, config.decay_min_score);
        assert!(engine.enabled);
    }

    #[test]
    fn test_compute_decay_zero_days() {
        let engine = DecayEngine::new(test_config());
        assert!((engine.compute_decay(0.0) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_compute_decay_half_life() {
        let engine = DecayEngine::new(test_config());
        assert!((engine.compute_decay(90.0) - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_compute_decay_one_year() {
        let engine = DecayEngine::new(test_config());
        // 365 / 90 ≈ 4.055 → 2^-4.055 ≈ 0.060 → clamped to min_score (0.1)
        let actual = engine.compute_decay(365.0);
        assert!((actual - engine.min_score).abs() < f64::EPSILON);
    }

    #[test]
    fn test_compute_decay_clamped_to_min_score() {
        let engine = DecayEngine::new(test_config());
        // Very large days_since → should clamp to min_score
        let decay = engine.compute_decay(1000.0);
        assert!((decay - engine.min_score).abs() < f64::EPSILON);
    }

    #[test]
    fn test_compute_decay_disabled() {
        let engine = DecayEngine::new(disabled_config());
        assert!((engine.compute_decay(1000.0) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_apply_decay_zero_days() {
        let engine = DecayEngine::new(test_config());
        assert!((engine.apply_decay(1.0, 0.0) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_apply_decay_half_life() {
        let engine = DecayEngine::new(test_config());
        assert!((engine.apply_decay(1.0, 90.0) - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_apply_decay_one_year() {
        let engine = DecayEngine::new(test_config());
        // 2^(-365/90) ≈ 0.060 → clamped to min_score (0.1)
        let actual = engine.apply_decay(1.0, 365.0);
        assert!((actual - engine.min_score).abs() < f64::EPSILON);
    }

    #[test]
    fn test_apply_decay_disabled() {
        let engine = DecayEngine::new(disabled_config());
        assert!((engine.apply_decay(1.0, 1000.0) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_apply_decay_with_rrf_score() {
        let engine = DecayEngine::new(test_config());
        let rrf_score = 0.8;
        let expected = rrf_score * 0.5;
        let actual = engine.apply_decay(rrf_score, 90.0);
        assert!((actual - expected).abs() < f64::EPSILON);
    }
}
