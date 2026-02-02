//! Embedding generation for memory vectors.
//!
//! Supports multiple embedding providers:
//! - OpenAI (text-embedding-3-small/large)
//! - Local hash-based (for testing/offline)

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::error::{CortexError, Result};
use cortex_common::create_default_client;

/// Embedding vector type.
pub type Embedding = Vec<f32>;

/// Embedder trait for generating vector embeddings.
#[async_trait::async_trait]
pub trait Embedder: Send + Sync + std::fmt::Debug {
    /// Generate embedding for a single text.
    async fn embed(&self, text: &str) -> Result<Embedding>;

    /// Generate embeddings for multiple texts (batch).
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Embedding>>;

    /// Get the embedding dimensions.
    fn dimensions(&self) -> usize;

    /// Get the model name.
    fn model_name(&self) -> &str;

    /// Get the provider name.
    fn provider(&self) -> &str;
}

/// Embedder configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedderConfig {
    /// Provider: "openai", "local".
    pub provider: String,
    /// Model name.
    pub model: String,
    /// API key (for OpenAI).
    pub api_key: Option<String>,
    /// API endpoint override.
    pub endpoint: Option<String>,
    /// Embedding dimensions.
    pub dimensions: usize,
    /// Batch size for bulk operations.
    pub batch_size: usize,
    /// Enable caching.
    pub cache_enabled: bool,
    /// Cache size limit.
    pub cache_size: usize,
}

impl Default for EmbedderConfig {
    fn default() -> Self {
        Self {
            provider: "local".to_string(),
            model: "local-hash".to_string(),
            api_key: None,
            endpoint: None,
            dimensions: 384,
            batch_size: 100,
            cache_enabled: true,
            cache_size: 10000,
        }
    }
}

impl EmbedderConfig {
    /// Create config for OpenAI.
    pub fn openai(api_key: impl Into<String>) -> Self {
        Self {
            provider: "openai".to_string(),
            model: "text-embedding-3-small".to_string(),
            api_key: Some(api_key.into()),
            endpoint: Some("https://api.openai.com/v1/embeddings".to_string()),
            dimensions: 1536,
            batch_size: 100,
            cache_enabled: true,
            cache_size: 10000,
        }
    }

    /// Create config for local hash-based embeddings.
    pub fn local() -> Self {
        Self {
            provider: "local".to_string(),
            model: "local-hash".to_string(),
            dimensions: 384,
            ..Default::default()
        }
    }
}

/// Embedding cache.
#[derive(Debug)]
struct EmbeddingCache {
    cache: HashMap<u64, Embedding>,
    max_size: usize,
    access_order: Vec<u64>,
}

impl EmbeddingCache {
    fn new(max_size: usize) -> Self {
        Self {
            cache: HashMap::new(),
            max_size,
            access_order: Vec::new(),
        }
    }

    fn get(&mut self, key: u64) -> Option<&Embedding> {
        if self.cache.contains_key(&key) {
            // Move to end of access order
            self.access_order.retain(|&k| k != key);
            self.access_order.push(key);
            self.cache.get(&key)
        } else {
            None
        }
    }

    fn insert(&mut self, key: u64, embedding: Embedding) {
        // Evict oldest if at capacity
        while self.cache.len() >= self.max_size && !self.access_order.is_empty() {
            if let Some(oldest) = self.access_order.first().copied() {
                self.access_order.remove(0);
                self.cache.remove(&oldest);
            }
        }

        self.cache.insert(key, embedding);
        self.access_order.push(key);
    }

    fn hash_text(text: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        hasher.finish()
    }
}

/// OpenAI embeddings provider.
#[derive(Debug)]
pub struct OpenAIEmbedder {
    config: EmbedderConfig,
    client: reqwest::Client,
    cache: RwLock<EmbeddingCache>,
}

impl OpenAIEmbedder {
    /// Create a new OpenAI embedder.
    pub fn new(config: EmbedderConfig) -> Result<Self> {
        let api_key = config
            .api_key
            .as_ref()
            .ok_or_else(|| CortexError::config("OpenAI API key required"))?;

        if api_key.is_empty() {
            return Err(CortexError::config("OpenAI API key cannot be empty"));
        }

        Ok(Self {
            cache: RwLock::new(EmbeddingCache::new(config.cache_size)),
            config,
            client: create_default_client().map_err(|e| CortexError::Internal(e))?,
        })
    }
}

#[async_trait::async_trait]
impl Embedder for OpenAIEmbedder {
    async fn embed(&self, text: &str) -> Result<Embedding> {
        // Check cache
        let hash = EmbeddingCache::hash_text(text);
        if self.config.cache_enabled {
            let mut cache = self.cache.write().await;
            if let Some(cached) = cache.get(hash) {
                return Ok(cached.clone());
            }
        }

        let endpoint = self
            .config
            .endpoint
            .as_deref()
            .unwrap_or("https://api.openai.com/v1/embeddings");

        let api_key = self
            .config
            .api_key
            .as_ref()
            .ok_or_else(|| CortexError::config("API key not configured"))?;

        let response = self
            .client
            .post(endpoint)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "model": self.config.model,
                "input": text
            }))
            .send()
            .await
            .map_err(CortexError::Network)?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(CortexError::Provider(format!(
                "OpenAI embedding API error {}: {}",
                status, body
            )));
        }

        let result: OpenAIEmbeddingResponse = response
            .json()
            .await
            .map_err(|e| CortexError::Provider(format!("Failed to parse response: {}", e)))?;

        let embedding = result
            .data
            .into_iter()
            .next()
            .map(|d| d.embedding)
            .ok_or_else(|| CortexError::Provider("No embedding returned".to_string()))?;

        // Cache result
        if self.config.cache_enabled {
            self.cache.write().await.insert(hash, embedding.clone());
        }

        Ok(embedding)
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Embedding>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        // Check cache for all texts
        let mut results = vec![None; texts.len()];
        let mut uncached_indices = Vec::new();
        let mut uncached_texts = Vec::new();

        if self.config.cache_enabled {
            let mut cache = self.cache.write().await;
            for (i, text) in texts.iter().enumerate() {
                let hash = EmbeddingCache::hash_text(text);
                if let Some(cached) = cache.get(hash) {
                    results[i] = Some(cached.clone());
                } else {
                    uncached_indices.push(i);
                    uncached_texts.push(*text);
                }
            }
        } else {
            uncached_indices = (0..texts.len()).collect();
            uncached_texts = texts.to_vec();
        }

        // Fetch uncached in batches
        for batch_start in (0..uncached_texts.len()).step_by(self.config.batch_size) {
            let batch_end = (batch_start + self.config.batch_size).min(uncached_texts.len());
            let batch: Vec<_> = uncached_texts[batch_start..batch_end].to_vec();

            let endpoint = self
                .config
                .endpoint
                .as_deref()
                .unwrap_or("https://api.openai.com/v1/embeddings");

            let api_key = self
                .config
                .api_key
                .as_ref()
                .ok_or_else(|| CortexError::config("API key not configured"))?;

            let response = self
                .client
                .post(endpoint)
                .header("Authorization", format!("Bearer {}", api_key))
                .header("Content-Type", "application/json")
                .json(&serde_json::json!({
                    "model": self.config.model,
                    "input": batch
                }))
                .send()
                .await
                .map_err(CortexError::Network)?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                return Err(CortexError::Provider(format!(
                    "OpenAI embedding API error {}: {}",
                    status, body
                )));
            }

            let result: OpenAIEmbeddingResponse = response
                .json()
                .await
                .map_err(|e| CortexError::Provider(format!("Failed to parse response: {}", e)))?;

            // Store results in correct order
            let mut sorted_data = result.data;
            sorted_data.sort_by_key(|d| d.index);

            for (j, data) in sorted_data.into_iter().enumerate() {
                let idx = uncached_indices[batch_start + j];
                results[idx] = Some(data.embedding.clone());

                // Cache
                if self.config.cache_enabled {
                    let hash = EmbeddingCache::hash_text(uncached_texts[batch_start + j]);
                    self.cache.write().await.insert(hash, data.embedding);
                }
            }
        }

        results
            .into_iter()
            .map(|r| r.ok_or_else(|| CortexError::Provider("Missing embedding".to_string())))
            .collect()
    }

    fn dimensions(&self) -> usize {
        self.config.dimensions
    }

    fn model_name(&self) -> &str {
        &self.config.model
    }

    fn provider(&self) -> &str {
        "openai"
    }
}

/// OpenAI embedding response.
#[derive(Debug, Deserialize)]
struct OpenAIEmbeddingResponse {
    data: Vec<OpenAIEmbeddingData>,
}

#[derive(Debug, Deserialize)]
struct OpenAIEmbeddingData {
    embedding: Vec<f32>,
    index: usize,
}

/// Local hash-based embedder for testing/offline use.
#[derive(Debug)]
pub struct LocalEmbedder {
    config: EmbedderConfig,
}

impl LocalEmbedder {
    /// Create a new local embedder.
    pub fn new(config: EmbedderConfig) -> Self {
        Self { config }
    }

    /// Generate deterministic embedding from text hash.
    fn hash_embed(&self, text: &str) -> Embedding {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let hash = hasher.finish();

        // Generate pseudo-random embedding from hash
        let mut rng_state = hash;
        let embedding: Vec<f32> = (0..self.config.dimensions)
            .map(|_| {
                rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);
                (rng_state as f32 / u64::MAX as f32) * 2.0 - 1.0
            })
            .collect();

        // Normalize to unit vector
        let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            embedding.into_iter().map(|x| x / norm).collect()
        } else {
            embedding
        }
    }
}

#[async_trait::async_trait]
impl Embedder for LocalEmbedder {
    async fn embed(&self, text: &str) -> Result<Embedding> {
        Ok(self.hash_embed(text))
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Embedding>> {
        Ok(texts.iter().map(|t| self.hash_embed(t)).collect())
    }

    fn dimensions(&self) -> usize {
        self.config.dimensions
    }

    fn model_name(&self) -> &str {
        &self.config.model
    }

    fn provider(&self) -> &str {
        "local"
    }
}

/// Create embedder from config.
pub fn create_embedder(config: EmbedderConfig) -> Result<Arc<dyn Embedder>> {
    match config.provider.as_str() {
        "openai" => Ok(Arc::new(OpenAIEmbedder::new(config)?)),
        "local" | _ => Ok(Arc::new(LocalEmbedder::new(config))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_local_embedder() {
        let config = EmbedderConfig::local();
        let embedder = LocalEmbedder::new(config);

        let embedding = embedder.embed("hello world").await.unwrap();
        assert_eq!(embedding.len(), 384);

        // Same text should give same embedding
        let embedding2 = embedder.embed("hello world").await.unwrap();
        assert_eq!(embedding, embedding2);

        // Different text should give different embedding
        let embedding3 = embedder.embed("goodbye world").await.unwrap();
        assert_ne!(embedding, embedding3);
    }

    #[tokio::test]
    async fn test_local_embedder_normalized() {
        let config = EmbedderConfig::local();
        let embedder = LocalEmbedder::new(config);

        let embedding = embedder.embed("test").await.unwrap();

        // Check it's normalized (length ~= 1)
        let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_batch_embedding() {
        let config = EmbedderConfig::local();
        let embedder = LocalEmbedder::new(config);

        let texts = vec!["hello", "world", "test"];
        let embeddings = embedder.embed_batch(&texts).await.unwrap();

        assert_eq!(embeddings.len(), 3);
        for emb in &embeddings {
            assert_eq!(emb.len(), 384);
        }
    }

    #[test]
    fn test_embedding_cache() {
        let mut cache = EmbeddingCache::new(2);

        cache.insert(1, vec![1.0]);
        cache.insert(2, vec![2.0]);

        // Access 1 first, then 2 - makes 2 most recently used
        assert!(cache.get(1).is_some()); // access_order: [2, 1]
        assert!(cache.get(2).is_some()); // access_order: [1, 2]

        // Insert third, should evict LRU (1 is now oldest)
        cache.insert(3, vec![3.0]);

        // 2 was accessed more recently, so 1 should be evicted
        assert!(cache.get(2).is_some());
        assert!(cache.get(3).is_some());
    }
}
