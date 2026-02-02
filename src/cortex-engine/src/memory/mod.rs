//! Memory/RAG system for long-term storage and retrieval.
//!
//! This module provides:
//! - Long-term memory storage with embeddings
//! - Semantic search across memories
//! - Codebase indexing for RAG
//! - Context assembly for prompts
//! - Memory decay and pruning
//! - Session-specific vs global memories

pub mod context;
pub mod embedding;
pub mod indexer;
pub mod retriever;
pub mod store;

pub use context::{ContextAssembler, ContextConfig, RetrievedContext};
pub use embedding::{Embedder, EmbedderConfig, LocalEmbedder, OpenAIEmbedder};
pub use indexer::{CodeChunk, CodeIndexer, FileIndexer, IndexUpdate, IndexerConfig};
pub use retriever::{Retriever, RetrieverConfig, SearchQuery, SearchResult};
pub use store::{
    Memory, MemoryFilter, MemoryMetadata, MemoryQuery, MemoryScope, MemoryStore, MemoryStoreConfig,
    MemoryType, StorageBackend,
};

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::error::{CortexError, Result};

/// Central memory system coordinating all components.
#[derive(Debug)]
pub struct MemorySystem {
    /// Memory store for persistence.
    store: Arc<MemoryStore>,
    /// Embedder for generating vectors.
    embedder: Arc<dyn Embedder>,
    /// Retriever for similarity search.
    retriever: Arc<Retriever>,
    /// Code indexer.
    indexer: Option<Arc<RwLock<CodeIndexer>>>,
    /// Context assembler.
    context_assembler: ContextAssembler,
    /// Configuration.
    config: MemorySystemConfig,
}

/// Memory system configuration.
#[derive(Debug, Clone)]
pub struct MemorySystemConfig {
    /// Store configuration.
    pub store: MemoryStoreConfig,
    /// Embedder configuration.
    pub embedder: EmbedderConfig,
    /// Retriever configuration.
    pub retriever: RetrieverConfig,
    /// Indexer configuration.
    pub indexer: Option<IndexerConfig>,
    /// Context configuration.
    pub context: ContextConfig,
    /// Enable automatic memory decay.
    pub decay_enabled: bool,
    /// Decay half-life in hours.
    pub decay_half_life_hours: f32,
    /// Maximum memories to keep.
    pub max_memories: usize,
}

impl Default for MemorySystemConfig {
    fn default() -> Self {
        Self {
            store: MemoryStoreConfig::default(),
            embedder: EmbedderConfig::default(),
            retriever: RetrieverConfig::default(),
            indexer: None,
            context: ContextConfig::default(),
            decay_enabled: true,
            decay_half_life_hours: 168.0, // 1 week
            max_memories: 10000,
        }
    }
}

impl MemorySystem {
    /// Create a new memory system.
    pub async fn new(config: MemorySystemConfig) -> Result<Self> {
        let store = Arc::new(MemoryStore::new(config.store.clone()).await?);
        let embedder: Arc<dyn Embedder> = match config.embedder.provider.as_str() {
            "openai" => Arc::new(OpenAIEmbedder::new(config.embedder.clone())?),
            "local" | _ => Arc::new(LocalEmbedder::new(config.embedder.clone())),
        };

        let retriever = Arc::new(Retriever::new(
            store.clone(),
            embedder.clone(),
            config.retriever.clone(),
        ));

        let indexer = if let Some(indexer_config) = &config.indexer {
            Some(Arc::new(RwLock::new(CodeIndexer::new(
                store.clone(),
                embedder.clone(),
                indexer_config.clone(),
            ))))
        } else {
            None
        };

        let context_assembler = ContextAssembler::new(retriever.clone(), config.context.clone());

        Ok(Self {
            store,
            embedder,
            retriever,
            indexer,
            context_assembler,
            config,
        })
    }

    /// Store a new memory.
    pub async fn store_memory(&self, content: &str, memory_type: MemoryType) -> Result<Memory> {
        self.store_with_metadata(content, memory_type, MemoryMetadata::default())
            .await
    }

    /// Store a memory with metadata.
    pub async fn store_with_metadata(
        &self,
        content: &str,
        memory_type: MemoryType,
        metadata: MemoryMetadata,
    ) -> Result<Memory> {
        let embedding = self.embedder.embed(content).await?;
        let memory = Memory::new(content, embedding, memory_type, metadata);
        self.store.insert(memory.clone()).await?;

        // Check if we need to prune
        if self.store.count().await? > self.config.max_memories {
            self.prune().await?;
        }

        Ok(memory)
    }

    /// Search memories by semantic similarity.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        self.retriever.search(query, limit).await
    }

    /// Search with filters.
    pub async fn search_filtered(
        &self,
        query: &str,
        filter: MemoryFilter,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        self.retriever.search_filtered(query, filter, limit).await
    }

    /// Get relevant context for a prompt.
    pub async fn get_context(&self, query: &str) -> Result<RetrievedContext> {
        self.context_assembler.assemble(query).await
    }

    /// Get context with custom configuration.
    pub async fn get_context_with_config(
        &self,
        query: &str,
        config: ContextConfig,
    ) -> Result<RetrievedContext> {
        let assembler = ContextAssembler::new(self.retriever.clone(), config);
        assembler.assemble(query).await
    }

    /// Index a project directory.
    pub async fn index_project(&self, path: PathBuf) -> Result<IndexUpdate> {
        let indexer = self
            .indexer
            .as_ref()
            .ok_or_else(|| CortexError::config("Indexer not configured"))?;
        indexer.write().await.index_directory(path).await
    }

    /// Index a single file.
    pub async fn index_file(&self, path: PathBuf) -> Result<usize> {
        let indexer = self
            .indexer
            .as_ref()
            .ok_or_else(|| CortexError::config("Indexer not configured"))?;
        indexer.write().await.index_file(path).await
    }

    /// Start watching for file changes.
    pub async fn start_watcher(&self, path: PathBuf) -> Result<()> {
        let indexer = self
            .indexer
            .as_ref()
            .ok_or_else(|| CortexError::config("Indexer not configured"))?;
        indexer.write().await.start_watching(path).await
    }

    /// Stop the file watcher.
    pub async fn stop_watcher(&self) -> Result<()> {
        if let Some(indexer) = &self.indexer {
            indexer.write().await.stop_watching().await?;
        }
        Ok(())
    }

    /// Apply memory decay based on age.
    pub async fn apply_decay(&self) -> Result<usize> {
        if !self.config.decay_enabled {
            return Ok(0);
        }

        self.store
            .apply_decay(self.config.decay_half_life_hours)
            .await
    }

    /// Prune old/low-relevance memories.
    pub async fn prune(&self) -> Result<usize> {
        self.store.prune(self.config.max_memories).await
    }

    /// Clear all session-specific memories.
    pub async fn clear_session(&self, session_id: &str) -> Result<usize> {
        self.store
            .delete_by_filter(MemoryFilter {
                scope: Some(MemoryScope::Session(session_id.to_string())),
                ..Default::default()
            })
            .await
    }

    /// Clear all memories.
    pub async fn clear_all(&self) -> Result<()> {
        self.store.clear().await
    }

    /// Get memory statistics.
    pub async fn stats(&self) -> Result<MemoryStats> {
        let total = self.store.count().await?;
        let by_type = self.store.count_by_type().await?;
        let by_scope = self.store.count_by_scope().await?;
        let oldest = self.store.oldest_memory().await?;
        let newest = self.store.newest_memory().await?;

        Ok(MemoryStats {
            total_memories: total,
            by_type,
            by_scope,
            oldest_timestamp: oldest.map(|m| m.timestamp),
            newest_timestamp: newest.map(|m| m.timestamp),
            storage_size_bytes: self.store.storage_size().await?,
        })
    }

    /// Export memories for backup.
    pub async fn export(&self) -> Result<Vec<Memory>> {
        self.store.export().await
    }

    /// Import memories from backup.
    pub async fn import(&self, memories: Vec<Memory>) -> Result<usize> {
        self.store.import(memories).await
    }

    /// Get the underlying store.
    pub fn memory_store(&self) -> &Arc<MemoryStore> {
        &self.store
    }

    /// Get the embedder.
    pub fn embedder(&self) -> &Arc<dyn Embedder> {
        &self.embedder
    }

    /// Get the retriever.
    pub fn retriever(&self) -> &Arc<Retriever> {
        &self.retriever
    }
}

/// Memory system statistics.
#[derive(Debug, Clone)]
pub struct MemoryStats {
    /// Total number of memories.
    pub total_memories: usize,
    /// Count by memory type.
    pub by_type: std::collections::HashMap<MemoryType, usize>,
    /// Count by scope.
    pub by_scope: std::collections::HashMap<String, usize>,
    /// Oldest memory timestamp.
    pub oldest_timestamp: Option<chrono::DateTime<chrono::Utc>>,
    /// Newest memory timestamp.
    pub newest_timestamp: Option<chrono::DateTime<chrono::Utc>>,
    /// Total storage size in bytes.
    pub storage_size_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_memory_system_creation() {
        let config = MemorySystemConfig {
            store: MemoryStoreConfig {
                backend: StorageBackend::InMemory,
                ..Default::default()
            },
            embedder: EmbedderConfig {
                provider: "local".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };

        let system = MemorySystem::new(config).await;
        assert!(system.is_ok());
    }

    #[tokio::test]
    #[ignore = "Local hash embedder doesn't produce semantically similar embeddings"]
    async fn test_store_and_search() {
        let config = MemorySystemConfig {
            store: MemoryStoreConfig {
                backend: StorageBackend::InMemory,
                ..Default::default()
            },
            embedder: EmbedderConfig {
                provider: "local".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };

        let system = MemorySystem::new(config).await.unwrap();

        // Store a memory
        let memory = system
            .store_memory("The capital of France is Paris", MemoryType::Fact)
            .await
            .unwrap();

        assert!(!memory.id.is_nil());

        // Search for it
        let results = system
            .search("What is the capital of France?", 5)
            .await
            .unwrap();
        assert!(!results.is_empty());
        assert!(results[0].score > 0.5);
    }
}
