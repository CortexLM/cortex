//! Similarity search and retrieval.
//!
//! Provides semantic search across memories with:
//! - Configurable similarity thresholds
//! - Top-k retrieval
//! - Hybrid ranking (relevance + recency)
//! - Filtered search

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::embedding::Embedder;
use super::store::{Memory, MemoryFilter, MemoryScope, MemoryStore, MemoryType};
use crate::error::Result;

/// Retriever for semantic search.
#[derive(Debug)]
pub struct Retriever {
    /// Memory store.
    store: Arc<MemoryStore>,
    /// Embedder.
    embedder: Arc<dyn Embedder>,
    /// Configuration.
    config: RetrieverConfig,
}

/// Retriever configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrieverConfig {
    /// Number of results to return.
    pub top_k: usize,
    /// Minimum similarity threshold (0.0 - 1.0).
    pub similarity_threshold: f32,
    /// Weight for recency in ranking (0.0 - 1.0).
    pub recency_weight: f32,
    /// Weight for relevance score in ranking (0.0 - 1.0).
    pub relevance_weight: f32,
    /// Include memory content in results.
    pub include_content: bool,
    /// Maximum content length to return.
    pub max_content_length: usize,
    /// Enable re-ranking.
    pub rerank_enabled: bool,
}

impl Default for RetrieverConfig {
    fn default() -> Self {
        Self {
            top_k: 10,
            similarity_threshold: 0.5,
            recency_weight: 0.2,
            relevance_weight: 0.3,
            include_content: true,
            max_content_length: 2000,
            rerank_enabled: false,
        }
    }
}

/// Search query.
#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    /// Query text.
    pub text: String,
    /// Number of results.
    pub limit: Option<usize>,
    /// Filter by memory types.
    pub types: Option<Vec<MemoryType>>,
    /// Filter by scope.
    pub scope: Option<MemoryScope>,
    /// Minimum similarity.
    pub min_similarity: Option<f32>,
    /// Maximum age in hours.
    pub max_age_hours: Option<f32>,
    /// Include only memories with these tags.
    pub tags: Option<Vec<String>>,
}

impl SearchQuery {
    /// Create a new search query.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }

    /// Set result limit.
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Filter by memory types.
    pub fn types(mut self, types: Vec<MemoryType>) -> Self {
        self.types = Some(types);
        self
    }

    /// Filter by scope.
    pub fn scope(mut self, scope: MemoryScope) -> Self {
        self.scope = Some(scope);
        self
    }

    /// Set minimum similarity.
    pub fn min_similarity(mut self, threshold: f32) -> Self {
        self.min_similarity = Some(threshold);
        self
    }

    /// Set maximum age.
    pub fn max_age(mut self, hours: f32) -> Self {
        self.max_age_hours = Some(hours);
        self
    }

    /// Filter by tags.
    pub fn tags(mut self, tags: Vec<String>) -> Self {
        self.tags = Some(tags);
        self
    }
}

/// Search result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// Memory ID.
    pub id: uuid::Uuid,
    /// Memory content.
    pub content: String,
    /// Similarity score (0.0 - 1.0).
    pub score: f32,
    /// Combined ranking score.
    pub rank_score: f32,
    /// Memory type.
    pub memory_type: MemoryType,
    /// Memory scope.
    pub scope: MemoryScope,
    /// Timestamp.
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Relevance score.
    pub relevance_score: f32,
    /// File path if applicable.
    pub file_path: Option<std::path::PathBuf>,
    /// Line range if applicable.
    pub line_range: Option<(usize, usize)>,
    /// Entity name if applicable.
    pub entity_name: Option<String>,
}

impl SearchResult {
    /// Create from memory and similarity score.
    fn from_memory(memory: Memory, similarity: f32, config: &RetrieverConfig) -> Self {
        // Calculate combined rank score
        let age_hours = memory.age_hours();
        let recency_score = 1.0 / (1.0 + age_hours / 24.0); // Decay over days

        let rank_score = similarity * (1.0 - config.recency_weight - config.relevance_weight)
            + recency_score * config.recency_weight
            + memory.relevance_score * config.relevance_weight;

        let content = if config.include_content {
            if memory.content.len() > config.max_content_length {
                format!("{}...", &memory.content[..config.max_content_length])
            } else {
                memory.content.clone()
            }
        } else {
            String::new()
        };

        Self {
            id: memory.id,
            content,
            score: similarity,
            rank_score,
            memory_type: memory.memory_type,
            scope: memory.scope,
            timestamp: memory.timestamp,
            relevance_score: memory.relevance_score,
            file_path: memory.metadata.file_path,
            line_range: memory.metadata.line_range,
            entity_name: memory.metadata.entity_name,
        }
    }
}

impl Retriever {
    /// Create a new retriever.
    pub fn new(
        store: Arc<MemoryStore>,
        embedder: Arc<dyn Embedder>,
        config: RetrieverConfig,
    ) -> Self {
        Self {
            store,
            embedder,
            config,
        }
    }

    /// Search memories by query text.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let search_query = SearchQuery::new(query).limit(limit);
        self.search_query(search_query).await
    }

    /// Search with filter.
    pub async fn search_filtered(
        &self,
        query: &str,
        filter: MemoryFilter,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let embedding = self.embedder.embed(query).await?;

        let results = self
            .store
            .search_similar(&embedding, limit * 2, Some(filter))
            .await?;

        let mut search_results: Vec<_> = results
            .into_iter()
            .filter(|(_, score)| *score >= self.config.similarity_threshold)
            .map(|(memory, score)| SearchResult::from_memory(memory, score, &self.config))
            .collect();

        // Sort by rank score
        search_results.sort_by(|a, b| {
            b.rank_score
                .partial_cmp(&a.rank_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        search_results.truncate(limit);

        Ok(search_results)
    }

    /// Search with full query options.
    pub async fn search_query(&self, query: SearchQuery) -> Result<Vec<SearchResult>> {
        let embedding = self.embedder.embed(&query.text).await?;
        let limit = query.limit.unwrap_or(self.config.top_k);
        let min_similarity = query
            .min_similarity
            .unwrap_or(self.config.similarity_threshold);

        // Build filter
        let filter = if query.types.is_some() || query.scope.is_some() || query.tags.is_some() {
            Some(MemoryFilter {
                types: query.types,
                scope: query.scope,
                min_age_hours: None,
                max_relevance: None,
                tags: query.tags,
            })
        } else {
            None
        };

        let results = self
            .store
            .search_similar(&embedding, limit * 2, filter)
            .await?;

        let mut search_results: Vec<_> = results
            .into_iter()
            .filter(|(memory, score)| {
                *score >= min_similarity
                    && query
                        .max_age_hours
                        .map_or(true, |max| memory.age_hours() <= max)
            })
            .map(|(memory, score)| SearchResult::from_memory(memory, score, &self.config))
            .collect();

        // Sort by rank score
        search_results.sort_by(|a, b| {
            b.rank_score
                .partial_cmp(&a.rank_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        search_results.truncate(limit);

        // Re-rank if enabled
        if self.config.rerank_enabled && !search_results.is_empty() {
            search_results = self.rerank(&query.text, search_results).await?;
        }

        Ok(search_results)
    }

    /// Search for code-related memories.
    pub async fn search_code(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let search_query = SearchQuery::new(query)
            .limit(limit)
            .types(vec![MemoryType::Code, MemoryType::FileContent]);
        self.search_query(search_query).await
    }

    /// Search for conversation history.
    pub async fn search_conversation(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let search_query = SearchQuery::new(query)
            .limit(limit)
            .types(vec![MemoryType::UserMessage, MemoryType::AssistantMessage]);
        self.search_query(search_query).await
    }

    /// Search within a specific session.
    pub async fn search_session(
        &self,
        query: &str,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let search_query = SearchQuery::new(query)
            .limit(limit)
            .scope(MemoryScope::Session(session_id.to_string()));
        self.search_query(search_query).await
    }

    /// Search recent memories.
    pub async fn search_recent(
        &self,
        query: &str,
        hours: f32,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let search_query = SearchQuery::new(query).limit(limit).max_age(hours);
        self.search_query(search_query).await
    }

    /// Get most similar memories to a given memory.
    pub async fn find_similar(
        &self,
        memory_id: uuid::Uuid,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let memory = self.store.get(memory_id).await?.ok_or_else(|| {
            crate::error::CortexError::NotFound(format!("Memory {} not found", memory_id))
        })?;

        let results = self
            .store
            .search_similar(&memory.embedding, limit + 1, None)
            .await?;

        // Filter out the source memory itself
        let search_results: Vec<_> = results
            .into_iter()
            .filter(|(m, _)| m.id != memory_id)
            .take(limit)
            .map(|(m, score)| SearchResult::from_memory(m, score, &self.config))
            .collect();

        Ok(search_results)
    }

    /// Re-rank results (placeholder for more sophisticated re-ranking).
    async fn rerank(&self, _query: &str, results: Vec<SearchResult>) -> Result<Vec<SearchResult>> {
        // Cross-encoder re-ranking planned for future implementation
        Ok(results)
    }

    /// Get retriever configuration.
    pub fn config(&self) -> &RetrieverConfig {
        &self.config
    }

    /// Update configuration.
    pub fn set_config(&mut self, config: RetrieverConfig) {
        self.config = config;
    }
}

/// Multi-query retriever that combines results from multiple queries.
#[derive(Debug)]
pub struct MultiQueryRetriever {
    retriever: Arc<Retriever>,
    num_queries: usize,
}

impl MultiQueryRetriever {
    /// Create a new multi-query retriever.
    pub fn new(retriever: Arc<Retriever>, num_queries: usize) -> Self {
        Self {
            retriever,
            num_queries,
        }
    }

    /// Search with query expansion.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        // Generate query variations
        let queries = self.expand_query(query);

        // Search with each query
        let mut all_results = Vec::new();
        for q in queries.iter().take(self.num_queries) {
            let results = self.retriever.search(q, limit).await?;
            all_results.extend(results);
        }

        // Deduplicate and re-rank
        let mut seen = std::collections::HashSet::new();
        let mut unique_results: Vec<_> = all_results
            .into_iter()
            .filter(|r| seen.insert(r.id))
            .collect();

        unique_results.sort_by(|a, b| {
            b.rank_score
                .partial_cmp(&a.rank_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        unique_results.truncate(limit);

        Ok(unique_results)
    }

    /// Expand query into variations.
    fn expand_query(&self, query: &str) -> Vec<String> {
        let mut queries = vec![query.to_string()];

        // Add lowercase version
        let lower = query.to_lowercase();
        if lower != query {
            queries.push(lower);
        }

        // Add version without punctuation
        let no_punct: String = query
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace())
            .collect();
        if no_punct != query {
            queries.push(no_punct);
        }

        queries
    }
}

/// Contextual retriever that considers conversation context.
#[derive(Debug)]
#[allow(dead_code)]
pub struct ContextualRetriever {
    retriever: Arc<Retriever>,
    context_weight: f32,
}

impl ContextualRetriever {
    /// Create a new contextual retriever.
    pub fn new(retriever: Arc<Retriever>, context_weight: f32) -> Self {
        Self {
            retriever,
            context_weight,
        }
    }

    /// Search with conversation context.
    pub async fn search_with_context(
        &self,
        query: &str,
        context: &[String],
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        // Combine query with recent context
        let combined_query = if context.is_empty() {
            query.to_string()
        } else {
            let context_str = context
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(" ");
            format!("{} {}", query, context_str)
        };

        // Search with combined query
        let mut results = self.retriever.search(&combined_query, limit * 2).await?;

        // Also search with original query
        let original_results = self.retriever.search(query, limit).await?;

        // Merge and re-rank
        let mut seen = std::collections::HashSet::new();
        for result in original_results {
            if seen.insert(result.id) {
                results.push(result);
            }
        }

        results.sort_by(|a, b| {
            b.rank_score
                .partial_cmp(&a.rank_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(limit);

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::embedding::{EmbedderConfig, LocalEmbedder};
    use crate::memory::store::{MemoryMetadata, MemoryStoreConfig, StorageBackend};

    async fn create_test_retriever() -> (Arc<MemoryStore>, Retriever) {
        let store = Arc::new(
            MemoryStore::new(MemoryStoreConfig {
                backend: StorageBackend::InMemory,
                ..Default::default()
            })
            .await
            .unwrap(),
        );

        let embedder: Arc<dyn Embedder> = Arc::new(LocalEmbedder::new(EmbedderConfig::local()));
        let retriever = Retriever::new(store.clone(), embedder, RetrieverConfig::default());

        (store, retriever)
    }

    #[tokio::test]
    #[ignore = "Local hash embedder doesn't produce semantically similar embeddings"]
    async fn test_basic_search() {
        let (store, retriever) = create_test_retriever().await;
        let embedder = Arc::new(LocalEmbedder::new(EmbedderConfig::local()));

        // Add some memories
        let embedding = embedder.embed("Rust programming language").await.unwrap();
        let memory = Memory::new(
            "Rust is a systems programming language",
            embedding,
            MemoryType::Note,
            MemoryMetadata::default(),
        );
        store.insert(memory).await.unwrap();

        let embedding = embedder.embed("Python scripting").await.unwrap();
        let memory = Memory::new(
            "Python is great for scripting",
            embedding,
            MemoryType::Note,
            MemoryMetadata::default(),
        );
        store.insert(memory).await.unwrap();

        // Search
        let results = retriever.search("programming", 5).await.unwrap();
        assert!(!results.is_empty());
    }

    #[tokio::test]
    async fn test_search_query_builder() {
        let query = SearchQuery::new("test query")
            .limit(10)
            .min_similarity(0.7)
            .types(vec![MemoryType::Code])
            .max_age(24.0);

        assert_eq!(query.text, "test query");
        assert_eq!(query.limit, Some(10));
        assert_eq!(query.min_similarity, Some(0.7));
        assert!(query.types.is_some());
        assert_eq!(query.max_age_hours, Some(24.0));
    }
}
