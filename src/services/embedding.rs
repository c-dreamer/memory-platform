//! Embedding service — local model (fastembed) or NVIDIA NIM API.
//!
//! Provides async embedding generation with:
//! - Local ONNX model (all-MiniLM-L6-v2, 384-dim)
//! - NVIDIA NIM API fallback
//! - LRU cache (1000 entries)

use crate::config::DEFAULT_EMBEDDING_DIM;
use crate::models::embedding::Embedding;
use anyhow::{Context, Result};
#[cfg(feature = "fastembed")]
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use lru::LruCache;
use reqwest::Client;
use std::fmt;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::Arc;
#[cfg(feature = "fastembed")]
use std::sync::Mutex as BlockingMutex;
use tokio::sync::Mutex;

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
    /// LRU cache size (number of entries).
    pub cache_size: usize,
}

#[cfg(feature = "fastembed")]
/// Local embedding backend using fastembed.
pub struct LocalEmbedding {
    model: Arc<BlockingMutex<TextEmbedding>>,
    cache: Arc<Mutex<LruCache<String, Embedding>>>,
}

#[cfg(feature = "fastembed")]
impl fmt::Debug for LocalEmbedding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalEmbedding")
            .field(
                "cache_size",
                &self.cache.try_lock().map(|c| c.len()).unwrap_or(0),
            )
            .finish()
    }
}

#[cfg(feature = "fastembed")]
impl LocalEmbedding {
    /// Create a new local embedding backend.
    pub async fn new(cache_size: usize) -> Result<Self> {
        let cache = Arc::new(Mutex::new(LruCache::new(
            NonZeroUsize::new(cache_size).unwrap_or(NonZeroUsize::new(1000).unwrap()),
        )));

        // Initialize model in a blocking task
        let model = tokio::task::spawn_blocking(|| -> Result<TextEmbedding> {
            let mut opts = InitOptions::default();
            opts.model_name = EmbeddingModel::AllMiniLML6V2;
            opts.show_download_progress = true;
            TextEmbedding::try_new(opts).context("Failed to initialize fastembed model")
        })
        .await
        .context("Failed to spawn blocking task for model init")??;

        Ok(Self {
            model: Arc::new(BlockingMutex::new(model)),
            cache,
        })
    }
}

#[cfg(feature = "fastembed")]
impl EmbeddingService for LocalEmbedding {
    fn embed(&self, text: &str) -> Pin<Box<dyn Future<Output = Result<Embedding>> + Send>> {
        let text = text.to_string();
        let model = Arc::clone(&self.model);
        let cache = Arc::clone(&self.cache);
        Box::pin(async move {
            if text.trim().is_empty() {
                return Ok(Embedding::new(vec![0.0; DEFAULT_EMBEDDING_DIM]));
            }

            // Check cache first
            {
                let mut cache = cache.lock().await;
                if let Some(cached) = cache.get(&text) {
                    return Ok(cached.clone());
                }
            }

            // Run ONNX inference in spawn_blocking
            let text_clone = text.clone();
            let model_inner = Arc::clone(&model);
            let embedding = tokio::task::spawn_blocking(move || {
                let mut model_lock = model_inner.lock().unwrap();
                model_lock
                    .embed(vec![text_clone], None)
                    .map(|embs| embs.into_iter().next())
                    .context("Failed to generate embedding")
            })
            .await
            .context("Failed to spawn blocking task for embedding")??;

            let embedding =
                Embedding::new(embedding.unwrap_or_else(|| vec![0.0; DEFAULT_EMBEDDING_DIM]));

            // Update cache
            {
                let mut cache = cache.lock().await;
                cache.put(text, embedding.clone());
            }

            Ok(embedding)
        })
    }
}

/// NVIDIA NIM embedding backend (HTTP API).
#[derive(Debug, Clone)]
pub struct NvidiaNimEmbedding {
    client: Client,
    api_url: String,
    api_key: String,
    model: String,
    cache: Arc<Mutex<LruCache<String, Embedding>>>,
}

impl NvidiaNimEmbedding {
    /// Create a new NVIDIA NIM embedding backend.
    pub fn new(api_url: String, api_key: String, model: String, cache_size: usize) -> Self {
        Self {
            client: Client::new(),
            api_url,
            api_key,
            model,
            cache: Arc::new(Mutex::new(LruCache::new(
                NonZeroUsize::new(cache_size).unwrap_or(NonZeroUsize::new(1000).unwrap()),
            ))),
        }
    }

    fn parse_embedding_response(data: serde_json::Value) -> Result<Embedding> {
        let mut embedding: Vec<f32> = data["data"][0]["embedding"]
            .as_array()
            .context("Invalid embedding format in NVIDIA NIM API response")?
            .iter()
            .map(|v| v.as_f64().unwrap_or_default() as f32)
            .collect();

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
        text: &str,
    ) -> Result<Embedding> {
        let resp = client
            .post(api_url)
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&serde_json::json!({
                "input": text,
                "model": model,
                "input_type": "query",
            }))
            .send()
            .await
            .context("Failed to send request to NVIDIA NIM API")?;

        let status = resp.status();
        if !status.is_success() {
            let error_text = resp.text().await.unwrap_or_default();
            anyhow::bail!("NVIDIA NIM API error ({}): {}", status, error_text);
        }

        let data: serde_json::Value = resp
            .json()
            .await
            .context("Failed to parse NVIDIA NIM API response")?;

        Self::parse_embedding_response(data)
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
        let cache = Arc::clone(&self.cache);
        Box::pin(async move {
            if text.trim().is_empty() {
                return Ok(Embedding::new(vec![0.0; DEFAULT_EMBEDDING_DIM]));
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
                Self::embed_remote_chunk(&client, &api_url, &api_key, &model, &chunks[0]).await?
            } else {
                let mut accumulator: Option<Vec<f32>> = None;
                let mut count = 0usize;

                for chunk in &chunks {
                    let chunk_embedding =
                        Self::embed_remote_chunk(&client, &api_url, &api_key, &model, chunk)
                            .await?;
                    let values = chunk_embedding.into_inner();

                    if let Some(existing) = accumulator.as_mut() {
                        for (dst, src) in existing.iter_mut().zip(values.iter()) {
                            *dst += *src;
                        }
                    } else {
                        accumulator = Some(values);
                    }
                    count += 1;
                }

                let mut values = accumulator.unwrap_or_else(|| vec![0.0; DEFAULT_EMBEDDING_DIM]);
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

/// Fallback embedding service — tries local (fastembed), falls back to NVIDIA NIM.
#[derive(Debug)]
pub struct FallbackEmbedding {
    primary: EmbeddingServiceFactory,
    fallback: EmbeddingServiceFactory,
}

impl FallbackEmbedding {
    pub fn new(primary: EmbeddingServiceFactory, fallback: EmbeddingServiceFactory) -> Self {
        Self { primary, fallback }
    }
}

impl EmbeddingService for FallbackEmbedding {
    fn embed(&self, text: &str) -> Pin<Box<dyn Future<Output = Result<Embedding>> + Send + '_>> {
        let text = text.to_string();
        let text2 = text.clone();
        Box::pin(async move {
            match self.primary.embed(&text).await {
                Ok(emb) => Ok(emb),
                Err(e) => {
                    tracing::warn!(
                        "Primary embedding failed ({:?}), falling back to NVIDIA NIM",
                        e
                    );
                    self.fallback.embed(&text2).await
                }
            }
        })
    }
}

/// Factory for embedding service.
pub enum EmbeddingServiceFactory {
    #[cfg(feature = "fastembed")]
    Local(LocalEmbedding),
    Nvidia(NvidiaNimEmbedding),
    Fallback(Box<FallbackEmbedding>),
}

impl fmt::Debug for EmbeddingServiceFactory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(feature = "fastembed")]
            Self::Local(_) => f.debug_tuple("Local").finish(),
            Self::Nvidia(_) => f.debug_tuple("Nvidia").finish(),
            Self::Fallback(_) => f.debug_tuple("Fallback").finish(),
        }
    }
}

impl EmbeddingServiceFactory {
    /// Create an embedding service based on configuration.
    ///
    /// When `model` is `"local"` with fastembed feature, and valid NVIDIA credentials
    /// are available, returns a `FallbackEmbedding` (local primary, NVIDIA fallback).
    /// When `model` is `"nvidia"`, uses NVIDIA NIM API directly.
    pub async fn new(config: EmbeddingConfig) -> Result<Self> {
        let cache_size = config.cache_size;
        match config.model.as_str() {
            "local" => {
                #[cfg(feature = "fastembed")]
                {
                    let local = LocalEmbedding::new(cache_size).await?;
                    // If NVIDIA credentials are available, wrap in fallback
                    if let (Some(url), Some(key)) = (config.nvidia_api_url, config.nvidia_api_key) {
                        if !url.is_empty() && !key.is_empty() {
                            let model_name = if config.nvidia_embedding_model.is_empty() {
                                "nvidia/nv-embedqa-e5-v5".to_string()
                            } else {
                                config.nvidia_embedding_model.clone()
                            };
                            let nvidia = NvidiaNimEmbedding::new(url, key, model_name, cache_size);
                            return Ok(Self::Fallback(Box::new(FallbackEmbedding::new(
                                EmbeddingServiceFactory::Local(local),
                                EmbeddingServiceFactory::Nvidia(nvidia),
                            ))));
                        }
                    }
                    Ok(Self::Local(local))
                }
                #[cfg(not(feature = "fastembed"))]
                {
                    let _ = config;
                    anyhow::bail!("Local embedding requires 'fastembed' feature")
                }
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
            #[cfg(feature = "fastembed")]
            Self::Local(service) => {
                let text = text.clone();
                Box::pin(async move { service.embed(&text).await })
            }
            Self::Nvidia(service) => Box::pin(async move { service.embed(&text).await }),
            Self::Fallback(service) => Box::pin(async move { service.embed(&text).await }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[cfg(feature = "fastembed")]
    async fn test_local_embedding() {
        let service = LocalEmbedding::new(1000)
            .await
            .expect("Failed to create local embedding");
        let embedding = service.embed("test").await.expect("Failed to embed");
        assert_eq!(embedding.as_vec().len(), DEFAULT_EMBEDDING_DIM);
        assert!(embedding.as_vec().iter().any(|&x| x != 0.0));
    }

    #[tokio::test]
    async fn test_nvidia_embedding() {
        let response = serde_json::json!({
            "data": [{"embedding": vec![0.1_f64; DEFAULT_EMBEDDING_DIM]}]
        });

        let embedding = NvidiaNimEmbedding::parse_embedding_response(response)
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
        );
        let embedding = service.embed("").await.expect("Failed to embed");
        assert_eq!(embedding.as_vec().len(), DEFAULT_EMBEDDING_DIM);
        assert!(embedding.as_vec().iter().all(|&x| x == 0.0));
    }
}
