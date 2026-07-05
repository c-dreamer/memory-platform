//! Memory decay engine.
//!
//! Implements Ebbinghaus-inspired half-life decay computed at query time.
//! Formula: score = max(min_score, 2^(-days_since / half_life))
//! apply_decay(rrf_score, days_since) = rrf_score * decay_factor

use std::sync::Arc;

/// Memory decay engine.
///
/// Reads configuration from `Config` and computes decay factors at query time.
/// If `enabled` is false, decay is always 1.0 (no-op).
#[derive(Debug, Clone)]
pub struct DecayEngine {
    /// Half-life in days for the Ebbinghaus decay formula.
    half_life_days: f64,
    /// Minimum score allowed after decay.
    min_score: f64,
    /// Whether decay is enabled.
    enabled: bool,
}

impl DecayEngine {
    /// Create a new decay engine from configuration.
    #[must_use]
    pub fn new(config: Arc<crate::config::Config>) -> Self {
        Self {
            half_life_days: config.decay_half_life_days,
            min_score: config.decay_min_score,
            enabled: config.decay_enabled,
        }
    }

    /// Compute the decay factor for a given number of days since access.
    ///
    /// Formula: 2^(-days_since / half_life_days)
    /// Returns the decay factor, clamped to `min_score`.
    #[must_use]
    pub fn compute_decay(&self, days_since_access: f64) -> f64 {
        if !self.enabled {
            return 1.0;
        }

        let decay_factor = 2.0f64.powf(-days_since_access / self.half_life_days);
        decay_factor.max(self.min_score)
    }

    /// Apply decay to an RRF score.
    ///
    /// Returns: rrf_score * decay_factor
    #[must_use]
    pub fn apply_decay(&self, rrf_score: f64, days_since: f64) -> f64 {
        if !self.enabled {
            return rrf_score;
        }

        rrf_score * self.compute_decay(days_since)
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
