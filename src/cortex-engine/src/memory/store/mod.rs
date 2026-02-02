//! Memory storage backend.
//!
//! Provides persistent storage for memories with support for:
//! - SQLite with vector search (sqlite-vec)
//! - File-based JSON persistence
//! - In-memory storage for sessions

mod config;
mod in_memory;
mod json_file;
mod query;
mod sqlite;
mod traits;
mod types;
mod utils;

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use uuid::Uuid;

use crate::error::Result;

// Re-export all public types
pub use config::{MemoryStoreConfig, StorageBackend};
pub use in_memory::InMemoryStorage;
pub use json_file::JsonFileStorage;
pub use query::{MemoryFilter, MemoryQuery};
pub use sqlite::SqliteStorage;
pub use traits::MemoryStorage;
pub use types::{Embedding, Memory, MemoryMetadata, MemoryScope, MemoryType};
pub use utils::cosine_similarity;

/// Memory store for persistence.
#[derive(Debug)]
pub struct MemoryStore {
    /// Configuration.
    config: MemoryStoreConfig,
    /// Storage implementation.
    storage: Arc<RwLock<Box<dyn MemoryStorage>>>,
}

impl MemoryStore {
    /// Create a new memory store.
    pub async fn new(config: MemoryStoreConfig) -> Result<Self> {
        let storage: Box<dyn MemoryStorage> = match &config.backend {
            StorageBackend::InMemory => Box::new(InMemoryStorage::new()),
            StorageBackend::JsonFile(path) => Box::new(JsonFileStorage::new(path.clone()).await?),
            StorageBackend::Sqlite(path) => Box::new(SqliteStorage::new(path.clone()).await?),
        };

        Ok(Self {
            config,
            storage: Arc::new(RwLock::new(storage)),
        })
    }

    /// Insert a memory.
    pub async fn insert(&self, memory: Memory) -> Result<()> {
        self.storage.write().await.insert(memory).await
    }

    /// Get a memory by ID.
    pub async fn get(&self, id: Uuid) -> Result<Option<Memory>> {
        self.storage.read().await.get(id).await
    }

    /// Update a memory.
    pub async fn update(&self, memory: Memory) -> Result<()> {
        self.storage.write().await.update(memory).await
    }

    /// Delete a memory.
    pub async fn delete(&self, id: Uuid) -> Result<bool> {
        self.storage.write().await.delete(id).await
    }

    /// Query memories.
    pub async fn query(&self, query: MemoryQuery) -> Result<Vec<Memory>> {
        self.storage.read().await.query(query).await
    }

    /// Get all memories (use with caution).
    pub async fn get_all(&self) -> Result<Vec<Memory>> {
        self.storage.read().await.get_all().await
    }

    /// Count memories.
    pub async fn count(&self) -> Result<usize> {
        self.storage.read().await.count().await
    }

    /// Count by memory type.
    pub async fn count_by_type(&self) -> Result<HashMap<MemoryType, usize>> {
        self.storage.read().await.count_by_type().await
    }

    /// Count by scope.
    pub async fn count_by_scope(&self) -> Result<HashMap<String, usize>> {
        self.storage.read().await.count_by_scope().await
    }

    /// Get oldest memory.
    pub async fn oldest_memory(&self) -> Result<Option<Memory>> {
        self.storage.read().await.oldest().await
    }

    /// Get newest memory.
    pub async fn newest_memory(&self) -> Result<Option<Memory>> {
        self.storage.read().await.newest().await
    }

    /// Delete memories by filter.
    pub async fn delete_by_filter(&self, filter: MemoryFilter) -> Result<usize> {
        self.storage.write().await.delete_by_filter(filter).await
    }

    /// Apply decay to all memories.
    pub async fn apply_decay(&self, half_life_hours: f32) -> Result<usize> {
        self.storage
            .write()
            .await
            .apply_decay(half_life_hours)
            .await
    }

    /// Prune memories to max count.
    pub async fn prune(&self, max_count: usize) -> Result<usize> {
        self.storage
            .write()
            .await
            .prune(max_count, self.config.prune_threshold)
            .await
    }

    /// Clear all memories.
    pub async fn clear(&self) -> Result<()> {
        self.storage.write().await.clear().await
    }

    /// Get storage size in bytes.
    pub async fn storage_size(&self) -> Result<u64> {
        self.storage.read().await.storage_size().await
    }

    /// Export all memories.
    pub async fn export(&self) -> Result<Vec<Memory>> {
        self.storage.read().await.export().await
    }

    /// Import memories.
    pub async fn import(&self, memories: Vec<Memory>) -> Result<usize> {
        self.storage.write().await.import(memories).await
    }

    /// Save to disk (for file backends).
    pub async fn save(&self) -> Result<()> {
        self.storage.write().await.save().await
    }

    /// Search by embedding similarity.
    pub async fn search_similar(
        &self,
        embedding: &Embedding,
        limit: usize,
        filter: Option<MemoryFilter>,
    ) -> Result<Vec<(Memory, f32)>> {
        self.storage
            .read()
            .await
            .search_similar(embedding, limit, filter)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_memory_creation() {
        let embedding = vec![0.1, 0.2, 0.3];
        let memory = Memory::new(
            "Test content",
            embedding.clone(),
            MemoryType::Note,
            MemoryMetadata::default(),
        );

        assert!(!memory.id.is_nil());
        assert_eq!(memory.content, "Test content");
        assert_eq!(memory.embedding, embedding);
        assert_eq!(memory.memory_type, MemoryType::Note);
        assert_eq!(memory.relevance_score, 1.0);
    }

    #[test]
    fn test_memory_decay() {
        let mut memory = Memory::new(
            "Test",
            vec![0.1],
            MemoryType::Note,
            MemoryMetadata::default(),
        );

        // Simulate age by modifying timestamp
        memory.timestamp = Utc::now() - chrono::Duration::hours(168); // 1 week old
        memory.apply_decay(168.0); // Half-life of 1 week

        // After one half-life, score should be ~0.5
        assert!(memory.relevance_score < 0.6);
        assert!(memory.relevance_score > 0.4);
    }

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.001);

        let c = vec![0.0, 1.0, 0.0];
        assert!(cosine_similarity(&a, &c).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_in_memory_storage() {
        let mut storage = InMemoryStorage::new();

        let memory = Memory::new(
            "Test",
            vec![0.1, 0.2],
            MemoryType::Note,
            MemoryMetadata::default(),
        );
        let id = memory.id;

        storage.insert(memory).await.unwrap();
        assert_eq!(storage.count().await.unwrap(), 1);

        let retrieved = storage.get(id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().content, "Test");

        storage.delete(id).await.unwrap();
        assert_eq!(storage.count().await.unwrap(), 0);
    }
}
