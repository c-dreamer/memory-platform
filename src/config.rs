//! Application configuration via environment variables.
//!
//! Mirrors the Python `config.py` Settings class. Reads from environment
//! variables with `.env` file support via `dotenvy`. All fields have
//! sensible defaults matching the Docker Compose development setup.

use std::env;
use std::fmt;

/// Errors that can occur during configuration loading.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to parse environment variable '{key}': {source}")]
    Parse {
        key: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// Default embedding dimension used as fallback when Config is not available.
pub const DEFAULT_EMBEDDING_DIM: usize = 384;

/// Application configuration loaded from environment variables.
///
/// Every field maps to an environment variable (see `.env.example`).
/// Defaults are provided for local development with Docker Compose.
#[derive(Clone)]
pub struct Config {
    // --- Database ---
    pub database_url: String,
    pub redis_url: String,
    pub neo4j_uri: String,
    pub neo4j_user: String,
    pub neo4j_password: String,

    // --- API ---
    pub(crate) api_key: String,
    pub api_port: u16,

    // --- Embeddings ---
    pub embedding_model: String,
    pub embedding_dim: usize,
    pub nvidia_api_url: String,
    pub nvidia_api_key: String,
    pub openai_api_key: String,

    // --- Obsidian ---
    pub vault_path: String,
    pub obsidian_api_url: String,
    pub obsidian_api_key: String,

    // --- Cache TTLs (seconds) ---
    pub cache_ttl_short: u64,
    pub cache_ttl_medium: u64,
    pub cache_ttl_long: u64,

    // --- Hybrid Search ---
    pub search_default_mode: String,
    pub rrf_k: u32,
    pub rrf_vector_weight: f64,
    pub rrf_keyword_weight: f64,

    // --- Memory Decay (Ebbinghaus-inspired) ---
    pub decay_enabled: bool,
    pub decay_half_life_days: f64,
    pub decay_min_score: f64,
    pub decay_apply_to_search: bool,
    // Coherence-weighted scoring weights (α, β, γ)
    pub coherence_weight_recency: f64,
    pub coherence_weight_frequency: f64,
    pub coherence_weight_semantic: f64,
    pub coherence_frequency_threshold: f64,

    // --- Chunking ---
    pub chunk_size: usize,
    pub chunk_overlap: usize,
    pub max_chunks_per_doc: usize,

    // --- NVIDIA embedding model ---
    pub nvidia_embedding_model: String,

    // --- Logging ---
    pub rust_log: String,

    // --- Embedding Cache ---
    pub embedding_cache_size: usize,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("database_url", &self.database_url)
            .field("redis_url", &self.redis_url)
            .field("neo4j_uri", &self.neo4j_uri)
            .field("neo4j_user", &self.neo4j_user)
            .field("neo4j_password", &self.neo4j_password)
            .field("api_key", &"***redacted***")
            .field("api_port", &self.api_port)
            .field("embedding_model", &self.embedding_model)
            .field("embedding_dim", &self.embedding_dim)
            .field("nvidia_api_key", &"***redacted***")
            .field("openai_api_key", &"***redacted***")
            .field("vault_path", &self.vault_path)
            .field("obsidian_api_key", &"***redacted***")
            .field("cache_ttl_short", &self.cache_ttl_short)
            .field("search_default_mode", &self.search_default_mode)
            .field("decay_enabled", &self.decay_enabled)
            .field("embedding_cache_size", &self.embedding_cache_size)
            .finish()
    }
}

impl Config {
    /// Load configuration from environment variables, with `.env` file support.
    ///
    /// Calls `dotenvy::dotenv().ok()` to load a `.env` file if present,
    /// then reads each variable with `std::env::var()`, falling back to
    /// the documented default when the variable is absent.
    ///
    /// # Errors
    ///
    /// Returns `ConfigError::Parse` if a typed value (integer, float, bool)
    /// is present but cannot be parsed.
    pub fn from_env() -> Result<Self, ConfigError> {
        // Load .env file if present; ignore errors (file may not exist).
        dotenvy::dotenv().ok();

        Ok(Self {
            // --- Database ---
            database_url: env_var("DATABASE_URL").unwrap_or_else(|| {
                "postgresql://memory:password@memory-postgres:5432/memory".into()
            }),
            redis_url: env_var("REDIS_URL").unwrap_or_else(|| "redis://memory-redis:6379/0".into()),
            neo4j_uri: env_var("NEO4J_URI").unwrap_or_else(|| "bolt://memory-neo4j:7687".into()),
            neo4j_user: env_var("NEO4J_USER").unwrap_or_else(|| "neo4j".into()),
            neo4j_password: env_var("NEO4J_PASSWORD").unwrap_or_default(),

            // --- API ---
            api_key: env_var("API_KEY").unwrap_or_default(),
            api_port: parse_env("API_PORT", 8000)?,

            // --- Embeddings ---
            embedding_model: env_var("EMBEDDING_MODEL").unwrap_or_else(|| "local".into()),
            embedding_dim: parse_env("EMBEDDING_DIM", 384)?,
            nvidia_api_url: env_var("NVIDIA_API_URL")
                .unwrap_or_else(|| "https://api.nvcf.nvidia.com/v2/nvcf/pexec/functions".into()),
            nvidia_api_key: env_var("NVIDIA_API_KEY").unwrap_or_default(),
            openai_api_key: env_var("OPENAI_API_KEY").unwrap_or_default(),

            // --- Obsidian ---
            vault_path: env_var("VAULT_PATH").unwrap_or_else(|| "/vault".into()),
            obsidian_api_url: env_var("OBSIDIAN_API_URL")
                .unwrap_or_else(|| "http://host.docker.internal:27124".into()),
            obsidian_api_key: env_var("OBSIDIAN_API_KEY").unwrap_or_default(),

            // --- Cache TTLs ---
            cache_ttl_short: parse_env("CACHE_TTL_SHORT", 300)?,
            cache_ttl_medium: parse_env("CACHE_TTL_MEDIUM", 3600)?,
            cache_ttl_long: parse_env("CACHE_TTL_LONG", 86400)?,

            // --- Hybrid Search ---
            search_default_mode: env_var("SEARCH_DEFAULT_MODE").unwrap_or_else(|| "hybrid".into()),
            rrf_k: parse_env("RRF_K", 20)?,
            rrf_vector_weight: parse_env("RRF_VECTOR_WEIGHT", 0.6)?,
            rrf_keyword_weight: parse_env("RRF_KEYWORD_WEIGHT", 0.4)?,

            // --- Memory Decay ---
            decay_enabled: parse_bool("DECAY_ENABLED", true)?,
            decay_half_life_days: parse_env("DECAY_HALF_LIFE_DAYS", 90.0)?,
            decay_min_score: parse_env("DECAY_MIN_SCORE", 0.1)?,
            decay_apply_to_search: parse_bool("DECAY_APPLY_TO_SEARCH", true)?,
            coherence_weight_recency: parse_env("COHERENCE_WEIGHT_RECENCY", 0.3)?,
            coherence_weight_frequency: parse_env("COHERENCE_WEIGHT_FREQUENCY", 0.2)?,
            coherence_weight_semantic: parse_env("COHERENCE_WEIGHT_SEMANTIC", 0.5)?,
            coherence_frequency_threshold: parse_env("COHERENCE_FREQUENCY_THRESHOLD", 5.0)?,

            // --- Chunking ---
            chunk_size: parse_env("CHUNK_SIZE", 512)?,
            chunk_overlap: parse_env("CHUNK_OVERLAP", 64)?,
            max_chunks_per_doc: parse_env("MAX_CHUNKS_PER_DOC", 100)?,

            // --- NVIDIA embedding model ---
            nvidia_embedding_model: env_var("NVIDIA_EMBEDDING_MODEL")
                .unwrap_or_else(|| "nvidia/llama-nemotron-embed-1b-v2".into()),

            // --- Logging ---
            rust_log: env_var("RUST_LOG").unwrap_or_else(|| "info".into()),

            // --- Embedding Cache ---
            embedding_cache_size: parse_env("EMBEDDING_CACHE_SIZE", 1000)?,
        })
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            database_url: "postgresql://memory:password@memory-postgres:5432/memory".into(),
            redis_url: "redis://memory-redis:6379/0".into(),
            neo4j_uri: "bolt://memory-neo4j:7687".into(),
            neo4j_user: "neo4j".into(),
            neo4j_password: String::new(),
            api_key: String::new(),
            api_port: 8000,
            embedding_model: "local".into(),
            embedding_dim: 384,
            nvidia_api_url: "https://api.nvcf.nvidia.com/v2/nvcf/pexec/functions".into(),
            nvidia_api_key: String::new(),
            openai_api_key: String::new(),
            vault_path: "/vault".into(),
            obsidian_api_url: "http://host.docker.internal:27124".into(),
            obsidian_api_key: String::new(),
            cache_ttl_short: 300,
            cache_ttl_medium: 3600,
            cache_ttl_long: 86400,
            search_default_mode: "hybrid".into(),
            rrf_k: 20,
            rrf_vector_weight: 0.6,
            rrf_keyword_weight: 0.4,
            decay_enabled: true,
            decay_half_life_days: 90.0,
            decay_min_score: 0.1,
            decay_apply_to_search: true,
            coherence_weight_recency: 0.3,
            coherence_weight_frequency: 0.2,
            coherence_weight_semantic: 0.5,
            coherence_frequency_threshold: 5.0,
            chunk_size: 512,
            chunk_overlap: 64,
            max_chunks_per_doc: 100,
            nvidia_embedding_model: "nvidia/llama-nemotron-embed-1b-v2".into(),
            rust_log: "info".into(),
            embedding_cache_size: 1000,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Read an environment variable, returning `None` if it is absent or empty.
fn env_var(key: &str) -> Option<String> {
    match env::var(key) {
        Ok(val) if !val.is_empty() => Some(val),
        _ => None,
    }
}

/// Parse an environment variable into a typed value, falling back to `default`
/// when the variable is absent or empty.
fn parse_env<T>(key: &'static str, default: T) -> Result<T, ConfigError>
where
    T: std::str::FromStr,
    <T as std::str::FromStr>::Err: std::error::Error + Send + Sync + 'static,
{
    match env_var(key) {
        Some(raw) => raw.parse::<T>().map_err(|e| ConfigError::Parse {
            key,
            source: Box::new(e),
        }),
        None => Ok(default),
    }
}

/// Parse a boolean environment variable.
///
/// Accepts `"true"`, `"1"`, `"yes"` (case-insensitive) as `true`;
/// everything else is `false`. When the variable is absent, returns `default`.
fn parse_bool(key: &'static str, default: bool) -> Result<bool, ConfigError> {
    match env_var(key) {
        Some(raw) => {
            let lower = raw.to_lowercase();
            Ok(matches!(lower.as_str(), "true" | "1" | "yes"))
        }
        None => Ok(default),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Serialise env-var mutation so tests don't race.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    /// Helper: run a closure with a set of env vars temporarily set,
    /// then restore the original state.
    fn with_env_vars<F>(vars: &[(&str, &str)], f: F)
    where
        F: FnOnce(),
    {
        let _guard = ENV_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let original_cwd = env::current_dir().expect("current dir");
        let temp_cwd = unique_temp_dir();
        fs::create_dir_all(&temp_cwd).expect("create temp dir");
        env::set_current_dir(&temp_cwd).expect("set temp cwd");

        // Save originals
        let saved: Vec<(String, Option<String>)> = vars
            .iter()
            .map(|(k, _)| ((*k).to_string(), env::var(k).ok()))
            .collect();

        // Set test values
        for (k, v) in vars {
            env::set_var(k, v);
        }

        let result = catch_unwind(AssertUnwindSafe(f));

        // Restore originals
        for (k, orig) in saved {
            match orig {
                Some(val) => env::set_var(&k, val),
                None => env::remove_var(&k),
            }
        }

        env::set_current_dir(&original_cwd).expect("restore cwd");
        let _ = fs::remove_dir_all(&temp_cwd);

        if let Err(panic) = result {
            resume_unwind(panic);
        }
    }

    fn unique_temp_dir() -> PathBuf {
        let mut dir = env::temp_dir();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        dir.push(format!(
            "memory_platform_config_test_{}_{}",
            std::process::id(),
            stamp
        ));
        dir
    }

    // ------------------------------------------------------------------
    // Default
    // ------------------------------------------------------------------

    #[test]
    fn default_matches_documented_values() {
        let cfg = Config::default();
        assert_eq!(
            cfg.database_url,
            "postgresql://memory:password@memory-postgres:5432/memory"
        );
        assert_eq!(cfg.redis_url, "redis://memory-redis:6379/0");
        assert_eq!(cfg.neo4j_uri, "bolt://memory-neo4j:7687");
        assert_eq!(cfg.neo4j_user, "neo4j");
        assert_eq!(cfg.neo4j_password, "");
        assert_eq!(cfg.api_key, "");
        assert_eq!(cfg.api_port, 8000);
        assert_eq!(cfg.embedding_model, "local");
        assert_eq!(cfg.embedding_dim, 384);
        assert_eq!(cfg.nvidia_api_key, "");
        assert_eq!(cfg.openai_api_key, "");
        assert_eq!(cfg.vault_path, "/vault");
        assert_eq!(cfg.obsidian_api_url, "http://host.docker.internal:27124");
        assert_eq!(cfg.obsidian_api_key, "");
        assert_eq!(cfg.cache_ttl_short, 300);
        assert_eq!(cfg.cache_ttl_medium, 3600);
        assert_eq!(cfg.cache_ttl_long, 86400);
        assert_eq!(cfg.search_default_mode, "hybrid");
        assert_eq!(cfg.rrf_k, 20);
        assert!((cfg.rrf_vector_weight - 0.6).abs() < f64::EPSILON);
        assert!((cfg.rrf_keyword_weight - 0.4).abs() < f64::EPSILON);
        assert!(cfg.decay_enabled);
        assert!((cfg.decay_half_life_days - 90.0).abs() < f64::EPSILON);
        assert!((cfg.decay_min_score - 0.1).abs() < f64::EPSILON);
        assert!(cfg.decay_apply_to_search);
        assert_eq!(cfg.chunk_size, 512);
        assert_eq!(cfg.chunk_overlap, 64);
        assert_eq!(cfg.max_chunks_per_doc, 100);
        assert_eq!(cfg.nvidia_embedding_model, "nvidia/llama-nemotron-embed-1b-v2");
        assert_eq!(cfg.rust_log, "info");
        assert_eq!(cfg.embedding_cache_size, 1000);
    }

    // ------------------------------------------------------------------
    // from_env — defaults when no vars are set
    // ------------------------------------------------------------------

    #[test]
    fn from_env_uses_defaults_when_no_vars_set() {
        with_env_vars(&[], || {
            let cfg = Config::from_env().expect("from_env should succeed with no vars");
            let default = Config::default();
            assert_eq!(cfg.database_url, default.database_url);
            assert_eq!(cfg.api_port, default.api_port);
            assert_eq!(cfg.embedding_dim, default.embedding_dim);
            assert_eq!(cfg.cache_ttl_short, default.cache_ttl_short);
            assert_eq!(cfg.rrf_k, default.rrf_k);
            assert!((cfg.rrf_vector_weight - default.rrf_vector_weight).abs() < f64::EPSILON);
            assert_eq!(cfg.decay_enabled, default.decay_enabled);
            assert_eq!(cfg.chunk_size, default.chunk_size);
            assert_eq!(cfg.rust_log, default.rust_log);
        });
    }

    // ------------------------------------------------------------------
    // from_env — custom values
    // ------------------------------------------------------------------

    #[test]
    fn from_env_reads_custom_string_values() {
        with_env_vars(
            &[
                ("DATABASE_URL", "postgres://custom:5432/db"),
                ("EMBEDDING_MODEL", "openai"),
                ("SEARCH_DEFAULT_MODE", "vector"),
                ("RUST_LOG", "debug"),
            ],
            || {
                let cfg = Config::from_env().expect("from_env should succeed");
                assert_eq!(cfg.database_url, "postgres://custom:5432/db");
                assert_eq!(cfg.embedding_model, "openai");
                assert_eq!(cfg.search_default_mode, "vector");
                assert_eq!(cfg.rust_log, "debug");
            },
        );
    }

    #[test]
    fn from_env_reads_custom_integer_values() {
        with_env_vars(
            &[
                ("API_PORT", "9090"),
                ("EMBEDDING_DIM", "768"),
                ("CACHE_TTL_SHORT", "600"),
                ("RRF_K", "60"),
                ("CHUNK_SIZE", "1024"),
                ("CHUNK_OVERLAP", "128"),
                ("MAX_CHUNKS_PER_DOC", "200"),
            ],
            || {
                let cfg = Config::from_env().expect("from_env should succeed");
                assert_eq!(cfg.api_port, 9090);
                assert_eq!(cfg.embedding_dim, 768);
                assert_eq!(cfg.cache_ttl_short, 600);
                assert_eq!(cfg.rrf_k, 60);
                assert_eq!(cfg.chunk_size, 1024);
                assert_eq!(cfg.chunk_overlap, 128);
                assert_eq!(cfg.max_chunks_per_doc, 200);
            },
        );
    }

    #[test]
    fn from_env_reads_custom_float_values() {
        with_env_vars(
            &[
                ("RRF_VECTOR_WEIGHT", "0.75"),
                ("RRF_KEYWORD_WEIGHT", "0.25"),
                ("DECAY_HALF_LIFE_DAYS", "30.0"),
                ("DECAY_MIN_SCORE", "0.05"),
            ],
            || {
                let cfg = Config::from_env().expect("from_env should succeed");
                assert!((cfg.rrf_vector_weight - 0.75).abs() < f64::EPSILON);
                assert!((cfg.rrf_keyword_weight - 0.25).abs() < f64::EPSILON);
                assert!((cfg.decay_half_life_days - 30.0).abs() < f64::EPSILON);
                assert!((cfg.decay_min_score - 0.05).abs() < f64::EPSILON);
            },
        );
    }

    // ------------------------------------------------------------------
    // Boolean parsing
    // ------------------------------------------------------------------

    #[test]
    fn parse_bool_true_variants() {
        for val in &["true", "TRUE", "True", "1", "yes", "YES", "Yes"] {
            with_env_vars(&[("DECAY_ENABLED", val)], || {
                let cfg = Config::from_env().expect("from_env should succeed");
                assert!(cfg.decay_enabled, "expected true for '{val}'");
            });
        }
    }

    #[test]
    fn parse_bool_false_variants() {
        for val in &["false", "FALSE", "0", "no", "NO", "off", "anything_else"] {
            with_env_vars(&[("DECAY_ENABLED", val)], || {
                let cfg = Config::from_env().expect("from_env should succeed");
                assert!(!cfg.decay_enabled, "expected false for '{val}'");
            });
        }
    }

    #[test]
    fn parse_bool_default_when_absent() {
        with_env_vars(&[], || {
            let cfg = Config::from_env().expect("from_env should succeed");
            assert!(cfg.decay_enabled);
            assert!(cfg.decay_apply_to_search);
        });
    }

    // ------------------------------------------------------------------
    // Empty env var → default
    // ------------------------------------------------------------------

    #[test]
    fn empty_env_var_falls_back_to_default() {
        with_env_vars(
            &[
                ("API_PORT", ""),
                ("EMBEDDING_DIM", ""),
                ("DATABASE_URL", ""),
            ],
            || {
                let cfg = Config::from_env().expect("from_env should succeed");
                assert_eq!(cfg.api_port, 8000);
                assert_eq!(cfg.embedding_dim, 384);
                assert_eq!(
                    cfg.database_url,
                    "postgresql://memory:password@memory-postgres:5432/memory"
                );
            },
        );
    }

    // ------------------------------------------------------------------
    // Parse errors
    // ------------------------------------------------------------------

    #[test]
    fn parse_error_on_invalid_integer() {
        with_env_vars(&[("API_PORT", "not_a_number")], || {
            let err = Config::from_env().unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("API_PORT"), "error should mention key: {msg}");
            assert!(
                msg.contains("failed to parse"),
                "error should mention parse: {msg}"
            );
        });
    }

    #[test]
    fn parse_error_on_invalid_float() {
        with_env_vars(&[("RRF_VECTOR_WEIGHT", "abc")], || {
            let err = Config::from_env().unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("RRF_VECTOR_WEIGHT"),
                "error should mention key: {msg}"
            );
        });
    }

    // ------------------------------------------------------------------
    // All fields populated
    // ------------------------------------------------------------------

    #[test]
    fn from_env_reads_all_fields_custom() {
        with_env_vars(
            &[
                ("DATABASE_URL", "pg://test"),
                ("REDIS_URL", "redis://test"),
                ("NEO4J_URI", "bolt://test"),
                ("NEO4J_USER", "admin"),
                ("NEO4J_PASSWORD", "secret"),
                ("API_KEY", "key-123"),
                ("API_PORT", "3000"),
                ("EMBEDDING_MODEL", "nvidia"),
                ("EMBEDDING_DIM", "1024"),
                ("NVIDIA_API_KEY", "nv-key"),
                ("OPENAI_API_KEY", "oa-key"),
                ("VAULT_PATH", "/my-vault"),
                ("OBSIDIAN_API_URL", "http://obsidian:1234"),
                ("OBSIDIAN_API_KEY", "obs-key"),
                ("CACHE_TTL_SHORT", "60"),
                ("CACHE_TTL_MEDIUM", "600"),
                ("CACHE_TTL_LONG", "6000"),
                ("SEARCH_DEFAULT_MODE", "keyword"),
                ("RRF_K", "10"),
                ("RRF_VECTOR_WEIGHT", "0.8"),
                ("RRF_KEYWORD_WEIGHT", "0.2"),
                ("DECAY_ENABLED", "false"),
                ("DECAY_HALF_LIFE_DAYS", "45.0"),
                ("DECAY_MIN_SCORE", "0.05"),
                ("DECAY_APPLY_TO_SEARCH", "0"),
                ("COHERENCE_WEIGHT_RECENCY", "0.4"),
                ("COHERENCE_WEIGHT_FREQUENCY", "0.1"),
                ("COHERENCE_WEIGHT_SEMANTIC", "0.5"),
                ("COHERENCE_FREQUENCY_THRESHOLD", "3.0"),
                ("CHUNK_SIZE", "256"),
                ("CHUNK_OVERLAP", "32"),
                ("MAX_CHUNKS_PER_DOC", "50"),
                ("NVIDIA_EMBEDDING_MODEL", "nvidia/custom"),
                ("RUST_LOG", "trace"),
                ("EMBEDDING_CACHE_SIZE", "500"),
            ],
            || {
                let cfg = Config::from_env().expect("from_env should succeed");
                assert_eq!(cfg.database_url, "pg://test");
                assert_eq!(cfg.redis_url, "redis://test");
                assert_eq!(cfg.neo4j_uri, "bolt://test");
                assert_eq!(cfg.neo4j_user, "admin");
                assert_eq!(cfg.neo4j_password, "secret");
                assert_eq!(cfg.api_key, "key-123");
                assert_eq!(cfg.api_port, 3000);
                assert_eq!(cfg.embedding_model, "nvidia");
                assert_eq!(cfg.embedding_dim, 1024);
                assert_eq!(cfg.nvidia_api_key, "nv-key");
                assert_eq!(cfg.openai_api_key, "oa-key");
                assert_eq!(cfg.vault_path, "/my-vault");
                assert_eq!(cfg.obsidian_api_url, "http://obsidian:1234");
                assert_eq!(cfg.obsidian_api_key, "obs-key");
                assert_eq!(cfg.cache_ttl_short, 60);
                assert_eq!(cfg.cache_ttl_medium, 600);
                assert_eq!(cfg.cache_ttl_long, 6000);
                assert_eq!(cfg.search_default_mode, "keyword");
                assert_eq!(cfg.rrf_k, 10);
                assert!((cfg.rrf_vector_weight - 0.8).abs() < f64::EPSILON);
                assert!((cfg.rrf_keyword_weight - 0.2).abs() < f64::EPSILON);
                assert!(!cfg.decay_enabled);
                assert!((cfg.decay_half_life_days - 45.0).abs() < f64::EPSILON);
                assert!((cfg.decay_min_score - 0.05).abs() < f64::EPSILON);
                assert!(!cfg.decay_apply_to_search);
                assert!((cfg.coherence_weight_recency - 0.4).abs() < f64::EPSILON);
                assert!((cfg.coherence_weight_frequency - 0.1).abs() < f64::EPSILON);
                assert!((cfg.coherence_weight_semantic - 0.5).abs() < f64::EPSILON);
                assert!((cfg.coherence_frequency_threshold - 3.0).abs() < f64::EPSILON);
                assert_eq!(cfg.chunk_size, 256);
                assert_eq!(cfg.chunk_overlap, 32);
                assert_eq!(cfg.max_chunks_per_doc, 50);
                assert_eq!(cfg.nvidia_embedding_model, "nvidia/custom");
                assert_eq!(cfg.rust_log, "trace");
                assert_eq!(cfg.embedding_cache_size, 500);
            },
        );
    }
}
