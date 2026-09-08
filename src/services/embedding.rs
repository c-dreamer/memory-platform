//! Embedding service — NVIDIA NIM API.
//!
//! `fastembed`'s bundled ONNX models top out at 1024 dimensions and cannot
//! reach this schema's 2048, so it was removed rather than kept as a broken
//! "local" option — see docs/WINDOWS_PORT_SYNTHESIS.md decision #4.
//!
//! Provides async embedding generation with:
//! - NVIDIA NIM API
//! - LRU cache (1000 entries)

#[cfg(test)]
use crate::config::DEFAULT_EMBEDDING_DIM;
use crate::models::embedding::Embedding;
use anyhow::{Context, Result};
use lru::LruCache;
use reqwest::Client;
use std::fmt;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

const EMBEDDING_REQUEST_TIMEOUT_SECS: u64 = 300;
const EMBEDDING_REQUEST_RETRIES: usize = 4;

/// Embedding service trait.
pub trait EmbeddingService: Send + Sync {
    /// Generate embedding for a single text string.
    fn embed(&self, text: &str) -> Pin<Box<dyn Future<Output = Result<Embedding>> + Send + '_>>;
}

/// Configuration for embedding service.
#[derive(Debug, Clone)]
pub struct EmbeddingConfig {
    /// Embedding model backend: "local" or "nvidia".
    pub model: String,
    /// NVIDIA NIM API URL.
    pub nvidia_api_url: Option<String>,
    /// NVIDIA API key.
    pub nvidia_api_key: Option<String>,
    /// NVIDIA embedding model name (e.g., "nvidia/nv-embed-v1").
    pub nvidia_embedding_model: String,
    /// Required output dimension. Provider responses are rejected if this differs.
    pub expected_dimension: usize,
    /// LRU cache size (number of entries).
    pub cache_size: usize,
}

/// NVIDIA NIM embedding backend (HTTP API).
#[derive(Debug, Clone)]
pub struct NvidiaNimEmbedding {
    client: Client,
    api_url: String,
    api_key: String,
    model: String,
    expected_dimension: usize,
    cache: Arc<Mutex<LruCache<String, Embedding>>>,
}

impl NvidiaNimEmbedding {
    /// Create a new NVIDIA NIM embedding backend.
    pub fn new(
        api_url: String,
        api_key: String,
        model: String,
        cache_size: usize,
        expected_dimension: usize,
    ) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(EMBEDDING_REQUEST_TIMEOUT_SECS))
                .build()
                .expect("valid embedding HTTP client configuration"),
            api_url,
            api_key,
            model,
            expected_dimension,
            cache: Arc::new(Mutex::new(LruCache::new(
                NonZeroUsize::new(cache_size).unwrap_or(NonZeroUsize::new(1000).unwrap()),
            ))),
        }
    }

    fn parse_embedding_response(
        data: serde_json::Value,
        expected_dimension: usize,
    ) -> Result<Embedding> {
        let mut embedding: Vec<f32> = data["data"][0]["embedding"]
            .as_array()
            .context("Invalid embedding format in NVIDIA NIM API response")?
            .iter()
            .map(|v| v.as_f64().unwrap_or_default() as f32)
            .collect();

        if embedding.len() != expected_dimension {
            anyhow::bail!(
                "embedding provider returned {} dimensions; expected {}",
                embedding.len(),
                expected_dimension
            );
        }

        // Normalize embedding
        let norm = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            embedding.iter_mut().for_each(|x| *x /= norm);
        }

        Ok(Embedding::new(embedding))
    }

    async fn embed_remote_chunk(
        client: &Client,
        api_url: &str,
        api_key: &str,
        model: &str,
        expected_dimension: usize,
        text: &str,
    ) -> Result<Embedding> {
        // Truncate to ~512 tokens (Unicode-safe, ~2048 bytes)
        let truncated_text = if text.chars().count() > 512 {
            text.chars().take(512).collect::<String>()
        } else {
            text.to_string()
        };
        for attempt in 0..=EMBEDDING_REQUEST_RETRIES {
            let response = client
                .post(api_url)
                .header("Authorization", format!("Bearer {}", api_key))
                .json(&serde_json::json!({
                    "input": truncated_text,
                    "model": model,
                    "input_type": "query",
                }))
                .send()
                .await;

            match response {
                Ok(resp) if resp.status().is_success() => {
                    let data: serde_json::Value = resp
                        .json()
                        .await
                        .context("Failed to parse NVIDIA NIM API response")?;
                    return Self::parse_embedding_response(data, expected_dimension);
                }
                Ok(resp) if !resp.status().is_server_error() => {
                    let status = resp.status();
                    let error_text = resp.text().await.unwrap_or_default();
                    anyhow::bail!("NVIDIA NIM API error ({}): {}", status, error_text);
                }
                Ok(resp) => {
                    tracing::warn!(attempt = attempt + 1, status = %resp.status(), "NVIDIA provider returned a retryable error");
                }
                Err(error) => {
                    tracing::warn!(attempt = attempt + 1, error = %error, "NVIDIA provider request failed; retrying");
                }
            }

            if attempt < EMBEDDING_REQUEST_RETRIES {
                tokio::time::sleep(Duration::from_secs(2_u64.saturating_pow(attempt as u32))).await;
            }
        }

        anyhow::bail!(
            "NVIDIA NIM API failed after {} attempts",
            EMBEDDING_REQUEST_RETRIES + 1
        )
    }

    fn split_for_embedding(text: &str, max_chars: usize) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        if chars.len() <= max_chars {
            return vec![text.to_string()];
        }

        chars
            .chunks(max_chars)
            .map(|chunk| chunk.iter().collect())
            .collect()
    }
}

impl EmbeddingService for NvidiaNimEmbedding {
    fn embed(&self, text: &str) -> Pin<Box<dyn Future<Output = Result<Embedding>> + Send + '_>> {
        let text = text.to_string();
        let client = self.client.clone();
        let api_url = self.api_url.clone();
        let api_key = self.api_key.clone();
        let model = self.model.clone();
        let expected_dimension = self.expected_dimension;
        let cache = Arc::clone(&self.cache);
        Box::pin(async move {
            if text.trim().is_empty() {
                anyhow::bail!("cannot embed empty text")
            }

            // Check cache first
            {
                let mut cache = cache.lock().await;
                if let Some(cached) = cache.get(&text) {
                    return Ok(cached.clone());
                }
            }

            let chunks = Self::split_for_embedding(&text, 4000);
            let embedding = if chunks.len() == 1 {
                Self::embed_remote_chunk(
                    &client,
                    &api_url,
                    &api_key,
                    &model,
                    expected_dimension,
                    &chunks[0],
                )
                .await?
            } else {
                let mut accumulator: Option<Vec<f32>> = None;
                let mut count = 0usize;

                for chunk in &chunks {
                    let chunk_embedding = Self::embed_remote_chunk(
                        &client,
                        &api_url,
                        &api_key,
                        &model,
                        expected_dimension,
                        chunk,
                    )
                    .await?;
                    let values = chunk_embedding.into_inner();
                    if values.len() != expected_dimension {
                        anyhow::bail!("embedding chunk dimension mismatch")
                    }

                    if let Some(existing) = accumulator.as_mut() {
                        for (dst, src) in existing.iter_mut().zip(values.iter()) {
                            *dst += *src;
                        }
                    } else {
                        accumulator = Some(values);
                    }
                    count += 1;
                }

                let mut values = accumulator.context("Embedding backend returned no vectors")?;
                if count > 1 {
                    let scale = 1.0 / count as f32;
                    for value in &mut values {
                        *value *= scale;
                    }
                }
                Embedding::new(values)
            };

            // Update cache
            {
                let mut cache = cache.lock().await;
                cache.put(text, embedding.clone());
            }

            Ok(embedding)
        })
    }
}

/// Factory for embedding service.
pub enum EmbeddingServiceFactory {
    Nvidia(NvidiaNimEmbedding),
}

impl fmt::Debug for EmbeddingServiceFactory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nvidia(_) => f.debug_tuple("Nvidia").finish(),
        }
    }
}

impl EmbeddingServiceFactory {
    /// Create an embedding service based on configuration.
    ///
    /// `model` `"local"` has no working backend yet — see the module doc
    /// comment — and fails clearly rather than silently reaching NVIDIA.
    /// `model` `"nvidia"` uses NVIDIA NIM API directly.
    pub async fn new(config: EmbeddingConfig) -> Result<Self> {
        let cache_size = config.cache_size.max(1);
        match config.model.as_str() {
            "local" => {
                anyhow::bail!(
                    "EMBEDDING_MODEL=local has no working backend yet (fastembed's models \
                     cannot reach this schema's 2048 dimensions); see \
                     docs/WINDOWS_PORT_SYNTHESIS.md decision #1"
                )
            }
            "nvidia" => {
                let api_url = config
                    .nvidia_api_url
                    .context("NVIDIA API URL is required for NVIDIA backend")?;
                let api_key = config
                    .nvidia_api_key
                    .context("NVIDIA API key is required for NVIDIA backend")?;
                Ok(Self::Nvidia(NvidiaNimEmbedding::new(
                    api_url,
                    api_key,
                    config.nvidia_embedding_model.clone(),
                    cache_size,
                    config.expected_dimension,
                )))
            }
            _ => anyhow::bail!("Unknown embedding model: {}", config.model),
        }
    }
}

impl EmbeddingService for EmbeddingServiceFactory {
    fn embed(&self, text: &str) -> Pin<Box<dyn Future<Output = Result<Embedding>> + Send + '_>> {
        let text = text.to_string();
        match self {
            Self::Nvidia(service) => Box::pin(async move { service.embed(&text).await }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_local_model_has_no_backend() {
        let config = EmbeddingConfig {
            model: "local".to_string(),
            nvidia_api_url: None,
            nvidia_api_key: None,
            nvidia_embedding_model: String::new(),
            expected_dimension: DEFAULT_EMBEDDING_DIM,
            cache_size: 10,
        };
        let err = EmbeddingServiceFactory::new(config)
            .await
            .expect_err("local model has no working backend yet");
        assert!(err.to_string().contains("no working backend"));
    }

    #[tokio::test]
    async fn test_nvidia_embedding() {
        let response = serde_json::json!({
            "data": [{"embedding": vec![0.1_f64; DEFAULT_EMBEDDING_DIM]}]
        });

        let embedding =
            NvidiaNimEmbedding::parse_embedding_response(response, DEFAULT_EMBEDDING_DIM)
                .expect("Failed to parse embedding");
        assert_eq!(embedding.as_vec().len(), DEFAULT_EMBEDDING_DIM);
        assert!(embedding.as_vec().iter().all(|x| x.is_finite()));
    }

    #[tokio::test]
    async fn test_empty_text() {
        let service = NvidiaNimEmbedding::new(
            "http://fake.url".to_string(),
            "fake-api-key".to_string(),
            "nv-embed-qa".to_string(),
            1000,
            DEFAULT_EMBEDDING_DIM,
        );
        assert!(service.embed("").await.is_err());
    }
}
