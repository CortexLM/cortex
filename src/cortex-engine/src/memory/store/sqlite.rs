//! SQLite storage implementation with vector search.

use std::collections::HashMap;
use std::path::PathBuf;

use uuid::Uuid;

use crate::error::{CortexError, Result};

use super::query::{MemoryFilter, MemoryQuery};
use super::traits::MemoryStorage;
use super::types::{Embedding, Memory, MemoryType};
use super::utils::cosine_similarity;

/// SQLite storage implementation with vector search.
#[derive(Debug)]
pub struct SqliteStorage {
    path: PathBuf,
    memories: HashMap<Uuid, Memory>,
    dirty: bool,
}

impl SqliteStorage {
    /// Create a new SQLite storage.
    pub async fn new(path: PathBuf) -> Result<Self> {
        // For now, use JSON serialization for SQLite as well
        // In production, you'd use rusqlite with sqlite-vec extension
        let memories = if path.exists() {
            let content = tokio::fs::read_to_string(&path).await.unwrap_or_default();
            if content.is_empty() {
                HashMap::new()
            } else {
                let list: Vec<Memory> = serde_json::from_str(&content).unwrap_or_default();
                list.into_iter().map(|m| (m.id, m)).collect()
            }
        } else {
            HashMap::new()
        };

        Ok(Self {
            path,
            memories,
            dirty: false,
        })
    }

    /// Check if a memory matches the given filter.
    fn matches_filter(&self, memory: &Memory, filter: &MemoryFilter) -> bool {
        if let Some(types) = &filter.types {
            if !types.contains(&memory.memory_type) {
                return false;
            }
        }
        if let Some(scope) = &filter.scope {
            if &memory.scope != scope {
                return false;
            }
        }
        if let Some(min_age) = filter.min_age_hours {
            if memory.age_hours() < min_age {
                return false;
            }
        }
        if let Some(max_relevance) = filter.max_relevance {
            if memory.relevance_score > max_relevance {
                return false;
            }
        }
        if let Some(tags) = &filter.tags {
            if !tags.iter().any(|t| memory.metadata.tags.contains(t)) {
                return false;
            }
        }
        true
    }

    /// Check if a memory matches the given query.
    fn matches_query(&self, memory: &Memory, query: &MemoryQuery) -> bool {
        if let Some(types) = &query.types {
            if !types.contains(&memory.memory_type) {
                return false;
            }
        }
        if let Some(scope) = &query.scope {
            if &memory.scope != scope {
                return false;
            }
        }
        if let Some(min_relevance) = query.min_relevance {
            if memory.relevance_score < min_relevance {
                return false;
            }
        }
        if let Some(max_age) = query.max_age_hours {
            if memory.age_hours() > max_age {
                return false;
            }
        }
        if let Some(tags) = &query.tags {
            if !tags.iter().any(|t| memory.metadata.tags.contains(t)) {
                return false;
            }
        }
        if let Some(prefix) = &query.file_path_prefix {
            if let Some(path) = &memory.metadata.file_path {
                if !path.starts_with(prefix) {
                    return false;
                }
            } else {
                return false;
            }
        }
        true
    }
}

#[async_trait::async_trait]
impl MemoryStorage for SqliteStorage {
    async fn insert(&mut self, memory: Memory) -> Result<()> {
        self.memories.insert(memory.id, memory);
        self.dirty = true;
        Ok(())
    }

    async fn get(&self, id: Uuid) -> Result<Option<Memory>> {
        Ok(self.memories.get(&id).cloned())
    }

    async fn update(&mut self, memory: Memory) -> Result<()> {
        if self.memories.contains_key(&memory.id) {
            self.memories.insert(memory.id, memory);
            self.dirty = true;
            Ok(())
        } else {
            Err(CortexError::NotFound(format!(
                "Memory {} not found",
                memory.id
            )))
        }
    }

    async fn delete(&mut self, id: Uuid) -> Result<bool> {
        let removed = self.memories.remove(&id).is_some();
        if removed {
            self.dirty = true;
        }
        Ok(removed)
    }

    async fn query(&self, query: MemoryQuery) -> Result<Vec<Memory>> {
        let mut results: Vec<_> = self
            .memories
            .values()
            .filter(|m| self.matches_query(m, &query))
            .cloned()
            .collect();

        results.sort_by(|a, b| {
            let score_a = a.relevance_score / (a.age_hours() + 1.0);
            let score_b = b.relevance_score / (b.age_hours() + 1.0);
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        if let Some(offset) = query.offset {
            results = results.into_iter().skip(offset).collect();
        }
        if let Some(limit) = query.limit {
            results.truncate(limit);
        }

        Ok(results)
    }

    async fn get_all(&self) -> Result<Vec<Memory>> {
        Ok(self.memories.values().cloned().collect())
    }

    async fn count(&self) -> Result<usize> {
        Ok(self.memories.len())
    }

    async fn count_by_type(&self) -> Result<HashMap<MemoryType, usize>> {
        let mut counts = HashMap::new();
        for memory in self.memories.values() {
            *counts.entry(memory.memory_type).or_insert(0) += 1;
        }
        Ok(counts)
    }

    async fn count_by_scope(&self) -> Result<HashMap<String, usize>> {
        let mut counts = HashMap::new();
        for memory in self.memories.values() {
            *counts.entry(memory.scope.to_string()).or_insert(0) += 1;
        }
        Ok(counts)
    }

    async fn oldest(&self) -> Result<Option<Memory>> {
        Ok(self.memories.values().min_by_key(|m| m.timestamp).cloned())
    }

    async fn newest(&self) -> Result<Option<Memory>> {
        Ok(self.memories.values().max_by_key(|m| m.timestamp).cloned())
    }

    async fn delete_by_filter(&mut self, filter: MemoryFilter) -> Result<usize> {
        let to_delete: Vec<_> = self
            .memories
            .iter()
            .filter(|(_, m)| self.matches_filter(m, &filter))
            .map(|(id, _)| *id)
            .collect();

        let count = to_delete.len();
        for id in to_delete {
            self.memories.remove(&id);
        }
        if count > 0 {
            self.dirty = true;
        }
        Ok(count)
    }

    async fn apply_decay(&mut self, half_life_hours: f32) -> Result<usize> {
        let mut count = 0;
        for memory in self.memories.values_mut() {
            let old_score = memory.relevance_score;
            memory.apply_decay(half_life_hours);
            if (old_score - memory.relevance_score).abs() > 0.001 {
                count += 1;
            }
        }
        if count > 0 {
            self.dirty = true;
        }
        Ok(count)
    }

    async fn prune(&mut self, max_count: usize, threshold: f32) -> Result<usize> {
        let expired: Vec<_> = self
            .memories
            .iter()
            .filter(|(_, m)| m.is_expired(threshold))
            .map(|(id, _)| *id)
            .collect();

        for id in &expired {
            self.memories.remove(id);
        }

        let mut removed = expired.len();
        while self.memories.len() > max_count {
            if let Some((id, _)) = self.memories.iter().min_by(|(_, a), (_, b)| {
                a.relevance_score
                    .partial_cmp(&b.relevance_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }) {
                let id = *id;
                self.memories.remove(&id);
                removed += 1;
            } else {
                break;
            }
        }

        if removed > 0 {
            self.dirty = true;
        }
        Ok(removed)
    }

    async fn clear(&mut self) -> Result<()> {
        self.memories.clear();
        self.dirty = true;
        Ok(())
    }

    async fn storage_size(&self) -> Result<u64> {
        if self.path.exists() {
            Ok(tokio::fs::metadata(&self.path).await?.len())
        } else {
            Ok(0)
        }
    }

    async fn export(&self) -> Result<Vec<Memory>> {
        Ok(self.memories.values().cloned().collect())
    }

    async fn import(&mut self, memories: Vec<Memory>) -> Result<usize> {
        let count = memories.len();
        for memory in memories {
            self.memories.insert(memory.id, memory);
        }
        self.dirty = true;
        Ok(count)
    }

    async fn save(&mut self) -> Result<()> {
        if self.dirty {
            let memories: Vec<_> = self.memories.values().cloned().collect();
            let content = serde_json::to_string_pretty(&memories)?;

            if let Some(parent) = self.path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }

            tokio::fs::write(&self.path, content).await?;
            self.dirty = false;
        }
        Ok(())
    }

    async fn search_similar(
        &self,
        embedding: &Embedding,
        limit: usize,
        filter: Option<MemoryFilter>,
    ) -> Result<Vec<(Memory, f32)>> {
        let mut results: Vec<_> = self
            .memories
            .values()
            .filter(|m| {
                filter
                    .as_ref()
                    .map(|f| self.matches_filter(m, f))
                    .unwrap_or(true)
            })
            .map(|m| {
                let score = cosine_similarity(embedding, &m.embedding);
                (m.clone(), score)
            })
            .collect();

        results.sort_by(|(_, a), (_, b)| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);

        Ok(results)
    }
}
