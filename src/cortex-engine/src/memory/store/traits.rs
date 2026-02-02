//! Storage trait for different backends.

use std::collections::HashMap;

use uuid::Uuid;

use crate::error::Result;

use super::query::{MemoryFilter, MemoryQuery};
use super::types::{Embedding, Memory, MemoryType};

/// Storage trait for different backends.
#[async_trait::async_trait]
pub trait MemoryStorage: Send + Sync + std::fmt::Debug {
    /// Insert a memory.
    async fn insert(&mut self, memory: Memory) -> Result<()>;

    /// Get a memory by ID.
    async fn get(&self, id: Uuid) -> Result<Option<Memory>>;

    /// Update a memory.
    async fn update(&mut self, memory: Memory) -> Result<()>;

    /// Delete a memory.
    async fn delete(&mut self, id: Uuid) -> Result<bool>;

    /// Query memories.
    async fn query(&self, query: MemoryQuery) -> Result<Vec<Memory>>;

    /// Get all memories.
    async fn get_all(&self) -> Result<Vec<Memory>>;

    /// Count memories.
    async fn count(&self) -> Result<usize>;

    /// Count by type.
    async fn count_by_type(&self) -> Result<HashMap<MemoryType, usize>>;

    /// Count by scope.
    async fn count_by_scope(&self) -> Result<HashMap<String, usize>>;

    /// Get oldest memory.
    async fn oldest(&self) -> Result<Option<Memory>>;

    /// Get newest memory.
    async fn newest(&self) -> Result<Option<Memory>>;

    /// Delete by filter.
    async fn delete_by_filter(&mut self, filter: MemoryFilter) -> Result<usize>;

    /// Apply decay.
    async fn apply_decay(&mut self, half_life_hours: f32) -> Result<usize>;

    /// Prune to max count.
    async fn prune(&mut self, max_count: usize, threshold: f32) -> Result<usize>;

    /// Clear all.
    async fn clear(&mut self) -> Result<()>;

    /// Get storage size.
    async fn storage_size(&self) -> Result<u64>;

    /// Export all.
    async fn export(&self) -> Result<Vec<Memory>>;

    /// Import memories.
    async fn import(&mut self, memories: Vec<Memory>) -> Result<usize>;

    /// Save to disk.
    async fn save(&mut self) -> Result<()>;

    /// Search by embedding similarity.
    async fn search_similar(
        &self,
        embedding: &Embedding,
        limit: usize,
        filter: Option<MemoryFilter>,
    ) -> Result<Vec<(Memory, f32)>>;
}
