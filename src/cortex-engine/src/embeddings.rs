//! Text embeddings and semantic search.
//!
//! Provides functionality for generating text embeddings,
//! computing similarity, and performing semantic search.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::error::{CortexError, Result};
use cortex_common::create_default_client;

/// Embedding vector.
pub type Embedding = Vec<f32>;

/// Embedding model configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Model name.
    pub model: String,
    /// Provider (openai, cohere, etc).
    pub provider: String,
    /// Dimension size.
    pub dimensions: usize,
    /// Batch size for bulk operations.
    pub batch_size: usize,
    /// Enable caching.
    pub cache_enabled: bool,
    /// API endpoint override.
    pub endpoint: Option<String>,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model: "text-embedding-3-small".to_string(),
            provider: "openai".to_string(),
            dimensions: 1536,
            batch_size: 100,
            cache_enabled: true,
            endpoint: None,
        }
    }
}

/// Embedding client trait.
#[async_trait::async_trait]
pub trait EmbeddingClient: Send + Sync {
    /// Generate embedding for text.
    async fn embed(&self, text: &str) -> Result<Embedding>;

    /// Generate embeddings for multiple texts.
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Embedding>>;

    /// Get embedding dimensions.
    fn dimensions(&self) -> usize;

    /// Get model name.
    fn model(&self) -> &str;
}

/// OpenAI embedding client.
pub struct OpenAIEmbeddings {
    /// Configuration.
    config: EmbeddingConfig,
    /// HTTP client.
    client: reqwest::Client,
    /// API key.
    api_key: String,
    /// Cache.
    cache: RwLock<EmbeddingCache>,
}

impl OpenAIEmbeddings {
    /// Create a new client.
    pub fn new(api_key: impl Into<String>, config: EmbeddingConfig) -> Self {
        Self {
            config,
            client: create_default_client().expect("HTTP client"),
            api_key: api_key.into(),
            cache: RwLock::new(EmbeddingCache::new(10000)),
        }
    }

    /// Create with default config.
    pub fn with_default(api_key: impl Into<String>) -> Self {
        Self::new(api_key, EmbeddingConfig::default())
    }
}

#[async_trait::async_trait]
impl EmbeddingClient for OpenAIEmbeddings {
    async fn embed(&self, text: &str) -> Result<Embedding> {
        // Check cache
        if self.config.cache_enabled
            && let Some(cached) = self.cache.read().await.get(text)
        {
            return Ok(cached);
        }

        let endpoint = self
            .config
            .endpoint
            .as_deref()
            .unwrap_or("https://api.openai.com/v1/embeddings");

        let response = self
            .client
            .post(endpoint)
            .header("Authorization", format!("Bearer {}", self.api_key))
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
                "Embedding API error {status}: {body}"
            )));
        }

        let result: EmbeddingResponse = response
            .json()
            .await
            .map_err(|e| CortexError::Provider(format!("Failed to parse response: {e}")))?;

        let embedding = result
            .data
            .into_iter()
            .next()
            .map(|d| d.embedding)
            .ok_or_else(|| CortexError::Provider("No embedding returned".to_string()))?;

        // Cache result
        if self.config.cache_enabled {
            self.cache
                .write()
                .await
                .insert(text.to_string(), embedding.clone());
        }

        Ok(embedding)
    }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Embedding>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        // Check cache for all texts
        let mut results = vec![None; texts.len()];
        let mut uncached_indices = Vec::new();
        let mut uncached_texts = Vec::new();

        if self.config.cache_enabled {
            let cache = self.cache.read().await;
            for (i, text) in texts.iter().enumerate() {
                if let Some(cached) = cache.get(text) {
                    results[i] = Some(cached);
                } else {
                    uncached_indices.push(i);
                    uncached_texts.push(text.clone());
                }
            }
        } else {
            uncached_indices = (0..texts.len()).collect();
            uncached_texts = texts.to_vec();
        }

        // Fetch uncached embeddings in batches
        for batch_start in (0..uncached_texts.len()).step_by(self.config.batch_size) {
            let batch_end = (batch_start + self.config.batch_size).min(uncached_texts.len());
            let batch: Vec<_> = uncached_texts[batch_start..batch_end].to_vec();

            let endpoint = self
                .config
                .endpoint
                .as_deref()
                .unwrap_or("https://api.openai.com/v1/embeddings");

            let response = self
                .client
                .post(endpoint)
                .header("Authorization", format!("Bearer {}", self.api_key))
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
                    "Embedding API error {status}: {body}"
                )));
            }

            let result: EmbeddingResponse = response
                .json()
                .await
                .map_err(|e| CortexError::Provider(format!("Failed to parse response: {e}")))?;

            // Store results
            for (j, data) in result.data.into_iter().enumerate() {
                let idx = uncached_indices[batch_start + j];
                results[idx] = Some(data.embedding.clone());

                // Cache
                if self.config.cache_enabled {
                    self.cache
                        .write()
                        .await
                        .insert(uncached_texts[batch_start + j].clone(), data.embedding);
                }
            }
        }

        // Convert to final results
        results
            .into_iter()
            .map(|r| r.ok_or_else(|| CortexError::Provider("Missing embedding".to_string())))
            .collect()
    }

    fn dimensions(&self) -> usize {
        self.config.dimensions
    }

    fn model(&self) -> &str {
        &self.config.model
    }
}

/// OpenAI embedding API response.
#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct EmbeddingData {
    embedding: Vec<f32>,
    index: usize,
}

/// Embedding cache.
#[derive(Debug)]
struct EmbeddingCache {
    embeddings: HashMap<String, Embedding>,
    max_size: usize,
}

impl EmbeddingCache {
    fn new(max_size: usize) -> Self {
        Self {
            embeddings: HashMap::new(),
            max_size,
        }
    }

    fn get(&self, key: &str) -> Option<Embedding> {
        self.embeddings.get(key).cloned()
    }

    fn insert(&mut self, key: String, embedding: Embedding) {
        if self.embeddings.len() >= self.max_size {
            // Simple eviction: remove first entry
            if let Some(first_key) = self.embeddings.keys().next().cloned() {
                self.embeddings.remove(&first_key);
            }
        }
        self.embeddings.insert(key, embedding);
    }
}

/// Compute cosine similarity between two embeddings.
pub fn cosine_similarity(a: &Embedding, b: &Embedding) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    dot / (norm_a * norm_b)
}

/// Compute Euclidean distance between two embeddings.
pub fn euclidean_distance(a: &Embedding, b: &Embedding) -> f32 {
    if a.len() != b.len() {
        return f32::MAX;
    }

    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).powi(2))
        .sum::<f32>()
        .sqrt()
}

/// Compute dot product between two embeddings.
pub fn dot_product(a: &Embedding, b: &Embedding) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }

    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// A document with embedding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddedDocument {
    /// Document ID.
    pub id: String,
    /// Document content.
    pub content: String,
    /// Embedding vector.
    pub embedding: Embedding,
    /// Metadata.
    pub metadata: HashMap<String, serde_json::Value>,
}

impl EmbeddedDocument {
    /// Create a new embedded document.
    pub fn new(id: impl Into<String>, content: impl Into<String>, embedding: Embedding) -> Self {
        Self {
            id: id.into(),
            content: content.into(),
            embedding,
            metadata: HashMap::new(),
        }
    }

    /// Add metadata.
    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }
}

/// Vector store for semantic search.
pub struct VectorStore {
    /// Documents.
    documents: RwLock<Vec<EmbeddedDocument>>,
    /// Embedding client.
    client: Arc<dyn EmbeddingClient>,
}

impl std::fmt::Debug for VectorStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VectorStore")
            .field("documents", &"<RwLock<Vec<EmbeddedDocument>>>")
            .field("client", &"<dyn EmbeddingClient>")
            .finish()
    }
}

impl VectorStore {
    /// Create a new vector store.
    pub fn new(client: Arc<dyn EmbeddingClient>) -> Self {
        Self {
            documents: RwLock::new(Vec::new()),
            client,
        }
    }

    /// Add a document.
    pub async fn add(&self, id: impl Into<String>, content: impl Into<String>) -> Result<()> {
        let content = content.into();
        let embedding = self.client.embed(&content).await?;

        let doc = EmbeddedDocument::new(id, content, embedding);
        self.documents.write().await.push(doc);

        Ok(())
    }

    /// Add multiple documents.
    pub async fn add_batch(&self, documents: Vec<(String, String)>) -> Result<()> {
        let texts: Vec<String> = documents.iter().map(|(_, c)| c.clone()).collect();
        let embeddings = self.client.embed_batch(&texts).await?;

        let mut docs = self.documents.write().await;
        for ((id, content), embedding) in documents.into_iter().zip(embeddings) {
            docs.push(EmbeddedDocument::new(id, content, embedding));
        }

        Ok(())
    }

    /// Search for similar documents.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let query_embedding = self.client.embed(query).await?;

        let docs = self.documents.read().await;
        let mut results: Vec<_> = docs
            .iter()
            .map(|doc| {
                let score = cosine_similarity(&query_embedding, &doc.embedding);
                SearchResult {
                    id: doc.id.clone(),
                    content: doc.content.clone(),
                    score,
                    metadata: doc.metadata.clone(),
                }
            })
            .collect();

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(limit);

        Ok(results)
    }

    /// Get document by ID.
    pub async fn get(&self, id: &str) -> Option<EmbeddedDocument> {
        self.documents
            .read()
            .await
            .iter()
            .find(|d| d.id == id)
            .cloned()
    }

    /// Remove document by ID.
    pub async fn remove(&self, id: &str) -> bool {
        let mut docs = self.documents.write().await;
        let len_before = docs.len();
        docs.retain(|d| d.id != id);
        docs.len() < len_before
    }

    /// Get total document count.
    pub async fn len(&self) -> usize {
        self.documents.read().await.len()
    }

    /// Check if store is empty.
    pub async fn is_empty(&self) -> bool {
        self.documents.read().await.is_empty()
    }

    /// Clear all documents.
    pub async fn clear(&self) {
        self.documents.write().await.clear();
    }
}

/// Search result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// Document ID.
    pub id: String,
    /// Document content.
    pub content: String,
    /// Similarity score (0-1).
    pub score: f32,
    /// Metadata.
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Chunker for splitting text into embeddable chunks.
#[derive(Debug, Clone)]
pub struct TextChunker {
    /// Maximum chunk size in characters.
    pub max_chars: usize,
    /// Overlap between chunks.
    pub overlap: usize,
    /// Split on sentence boundaries.
    pub sentence_boundary: bool,
}

impl Default for TextChunker {
    fn default() -> Self {
        Self {
            max_chars: 1000,
            overlap: 100,
            sentence_boundary: true,
        }
    }
}

impl TextChunker {
    /// Create a new chunker.
    pub fn new(max_chars: usize) -> Self {
        Self {
            max_chars,
            ..Default::default()
        }
    }

    /// Set overlap.
    pub fn with_overlap(mut self, overlap: usize) -> Self {
        self.overlap = overlap;
        self
    }

    /// Enable/disable sentence boundary splitting.
    pub fn sentence_boundary(mut self, enabled: bool) -> Self {
        self.sentence_boundary = enabled;
        self
    }

    /// Split text into chunks.
    pub fn chunk(&self, text: &str) -> Vec<TextChunk> {
        if text.len() <= self.max_chars {
            return vec![TextChunk {
                content: text.to_string(),
                start: 0,
                end: text.len(),
                index: 0,
            }];
        }

        let mut chunks = Vec::new();
        let mut start = 0;
        let mut index = 0;

        while start < text.len() {
            let mut end = (start + self.max_chars).min(text.len());

            // Adjust to sentence boundary if enabled
            if self.sentence_boundary
                && end < text.len()
                && let Some(sentence_end) = self.find_sentence_end(&text[start..end])
            {
                end = start + sentence_end;
            }

            chunks.push(TextChunk {
                content: text[start..end].to_string(),
                start,
                end,
                index,
            });

            // Move to next chunk with overlap
            start = if end >= text.len() {
                text.len()
            } else if end <= self.overlap {
                start + 1
            } else {
                (end - self.overlap).max(start + 1)
            };
            index += 1;
        }

        chunks
    }

    /// Find sentence end position.
    fn find_sentence_end(&self, text: &str) -> Option<usize> {
        let terminators = [". ", "! ", "? ", ".\n", "!\n", "?\n"];

        for (i, _) in text.char_indices().rev() {
            for term in &terminators {
                if text[i..].starts_with(term) {
                    return Some(i + term.len());
                }
            }
        }

        None
    }
}

/// A text chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextChunk {
    /// Chunk content.
    pub content: String,
    /// Start position in original text.
    pub start: usize,
    /// End position in original text.
    pub end: usize,
    /// Chunk index.
    pub index: usize,
}

/// Mock embedding client for testing.
pub struct MockEmbeddingClient {
    dimensions: usize,
}

impl MockEmbeddingClient {
    /// Create a new mock client.
    pub fn new(dimensions: usize) -> Self {
        Self { dimensions }
    }
}

#[async_trait::async_trait]
impl EmbeddingClient for MockEmbeddingClient {
    async fn embed(&self, text: &str) -> Result<Embedding> {
        // Generate deterministic embedding based on text hash
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let hash = hasher.finish();

        let mut rng_state = hash;
        let embedding: Vec<f32> = (0..self.dimensions)
            .map(|_| {
                rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);
                (rng_state as f32 / u64::MAX as f32) * 2.0 - 1.0
            })
            .collect();

        // Normalize
        let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        Ok(embedding.into_iter().map(|x| x / norm).collect())
    }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Embedding>> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed(text).await?);
        }
        Ok(results)
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn model(&self) -> &str {
        "mock"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.001);

        let c = vec![0.0, 1.0, 0.0];
        assert!(cosine_similarity(&a, &c).abs() < 0.001);
    }

    #[test]
    fn test_euclidean_distance() {
        let a = vec![0.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((euclidean_distance(&a, &b) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_text_chunker() {
        let chunker = TextChunker::new(100);
        let text = "This is a test. ".repeat(20);

        let chunks = chunker.chunk(&text);
        assert!(chunks.len() > 1);

        // Check all content is covered
        for chunk in &chunks {
            assert!(chunk.content.len() <= 100 + 50); // Allow some margin
        }
    }

    #[tokio::test]
    async fn test_mock_embedding_client() {
        let client = MockEmbeddingClient::new(128);

        let embedding = client.embed("hello world").await.unwrap();
        assert_eq!(embedding.len(), 128);

        // Same text should give same embedding
        let embedding2 = client.embed("hello world").await.unwrap();
        assert!((cosine_similarity(&embedding, &embedding2) - 1.0).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_vector_store() {
        let client = Arc::new(MockEmbeddingClient::new(128));
        let store = VectorStore::new(client);

        store.add("doc1", "Hello world").await.unwrap();
        store.add("doc2", "Goodbye world").await.unwrap();
        store
            .add("doc3", "Something completely different")
            .await
            .unwrap();

        let results = store.search("Hello", 2).await.unwrap();
        assert_eq!(results.len(), 2);

        // First result should be most similar to "Hello"
        assert!(results[0].score > results[1].score);
    }
}
