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
/// When `enabled` is false, `score_recency_frequency()` returns 1.0 (no-op).
#[derive(Debug, Clone)]
pub struct DecayEngine {
    half_life_days: f64,
    min_score: f64,
    enabled: bool,
    recency_weight: f64,
    frequency_weight: f64,
    semantic_weight: f64,
    frequency_threshold: f64,
    multiplier_floor: f64,
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
            multiplier_floor: config.decay_multiplier_floor.clamp(0.0, 1.0),
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

    /// Combined recency + frequency + importance score, for callers with no
    /// coherence (semantic similarity) signal available at the point of
    /// scoring (e.g. post-RRF-fusion re-ranking).
    ///
    /// Stored `importance` fills `semantic_weight`'s slot instead of being
    /// dropped: it's the closest available proxy signal, and reuses this
    /// method's existing renormalize-over-available-terms shape rather than
    /// silently discarding that share of the score.
    #[must_use]
    pub fn score_recency_frequency(
        &self,
        days_since_access: f64,
        access_count: f64,
        importance: f64,
    ) -> f64 {
        if !self.enabled {
            return 1.0;
        }
        let recency = self.compute_recency(days_since_access);
        let frequency = self.compute_frequency(access_count);
        let total_weight = self.recency_weight + self.frequency_weight + self.semantic_weight;
        let composite = if total_weight <= 0.0 {
            recency
        } else {
            (recency * self.recency_weight
                + frequency * self.frequency_weight
                + importance * self.semantic_weight)
                / total_weight
        };
        let composite = composite.clamp(0.0, 1.0);
        self.multiplier_floor + (1.0 - self.multiplier_floor) * composite
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
    fn test_compute_recency_zero_days() {
        let engine = DecayEngine::new(test_config());
        assert!((engine.compute_recency(0.0) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_compute_recency_half_life() {
        let engine = DecayEngine::new(test_config());
        assert!((engine.compute_recency(90.0) - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_compute_recency_clamped_to_min_score() {
        let engine = DecayEngine::new(test_config());
        // Very large days_since → should clamp to min_score
        let decay = engine.compute_recency(1000.0);
        assert!((decay - engine.min_score).abs() < f64::EPSILON);
    }

    #[test]
    fn test_compute_recency_disabled() {
        let engine = DecayEngine::new(disabled_config());
        assert!((engine.compute_recency(1000.0) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_score_recency_frequency_zero_days_max_access() {
        let engine = DecayEngine::new(test_config());
        // recency=1.0 (0 days), frequency=1.0 (access_count >= threshold),
        // importance=1.0 -> 1.0
        let actual = engine.score_recency_frequency(0.0, 100.0, 1.0);
        assert!((actual - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_score_recency_frequency_renormalizes_with_importance() {
        let engine = DecayEngine::new(test_config());
        // recency=0.5 (half-life), frequency=0.0 (never accessed), importance=0.8.
        // weights: recency=0.3, frequency=0.2, semantic(importance)=0.5 -> total 1.0.
        let composite = 0.5 * 0.3 + 0.0 * 0.2 + 0.8 * 0.5;
        let floor = engine.multiplier_floor;
        let expected = floor + (1.0 - floor) * composite;
        let actual = engine.score_recency_frequency(90.0, 0.0, 0.8);
        assert!((actual - expected).abs() < f64::EPSILON);
    }

    #[test]
    fn test_score_recency_frequency_disabled() {
        let engine = DecayEngine::new(disabled_config());
        assert!((engine.score_recency_frequency(1000.0, 0.0, 0.5) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_score_recency_frequency_bounded_by_multiplier_floor() {
        let config = test_config();
        let engine = DecayEngine::new(config.clone());
        // Best case: no elapsed time, unlimited access, max importance -> composite 1.0.
        let max = engine.score_recency_frequency(0.0, f64::MAX, 1.0);
        // Worst case: unlimited elapsed time, zero access, zero importance -> composite 0.0.
        let min = engine.score_recency_frequency(f64::MAX, 0.0, 0.0);
        assert!((max - 1.0).abs() < 1e-9);
        assert!(min >= config.decay_multiplier_floor - 1e-9);
        assert!(max / min <= 1.0 / config.decay_multiplier_floor + 1e-9);
    }

    #[test]
    fn test_score_recency_frequency_total_weight_zero_still_bounded() {
        let mut config = (*test_config()).clone();
        config.coherence_weight_recency = 0.0;
        config.coherence_weight_frequency = 0.0;
        config.coherence_weight_semantic = 0.0;
        let engine = DecayEngine::new(Arc::new(config.clone()));
        // total_weight <= 0.0 guard fires; result must still respect the floor,
        // not fall through to raw recency in [decay_min_score, 1.0].
        let actual = engine.score_recency_frequency(0.0, 100.0, 1.0);
        let floor = config.decay_multiplier_floor;
        let expected = floor + (1.0 - floor) * 1.0_f64.clamp(0.0, 1.0);
        assert!((actual - expected).abs() < 1e-9);
    }
}
