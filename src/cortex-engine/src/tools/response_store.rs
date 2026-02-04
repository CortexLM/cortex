//! Tool response storage with bounded capacity and automatic cleanup.
//!
//! This module provides a bounded storage for tool execution results that:
//! - Limits maximum number of stored responses to prevent unbounded memory growth
//! - Removes entries when they are consumed (read and take)
//! - Periodically cleans up stale entries based on TTL
//!
//! Fixes #5292 (unbounded growth) and #5293 (missing removal on read).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use tracing::debug;

use crate::tools::spec::ToolResult;

/// Maximum number of responses to store before eviction.
/// This prevents unbounded memory growth from accumulated tool responses.
pub const MAX_STORE_SIZE: usize = 500;

/// Default time-to-live for stored responses (5 minutes).
pub const DEFAULT_TTL: Duration = Duration::from_secs(300);

/// Interval for periodic cleanup of stale entries (1 minute).
pub const CLEANUP_INTERVAL: Duration = Duration::from_secs(60);

/// A stored tool response with metadata.
#[derive(Debug, Clone)]
pub struct StoredResponse {
    /// The tool execution result.
    pub result: ToolResult,
    /// Tool name that produced this result.
    pub tool_name: String,
    /// When the response was stored.
    pub stored_at: Instant,
    /// Whether this response has been read (but not yet consumed).
    pub read: bool,
}

impl StoredResponse {
    /// Create a new stored response.
    pub fn new(tool_name: impl Into<String>, result: ToolResult) -> Self {
        Self {
            result,
            tool_name: tool_name.into(),
            stored_at: Instant::now(),
            read: false,
        }
    }

    /// Check if the response has expired.
    pub fn is_expired(&self, ttl: Duration) -> bool {
        self.stored_at.elapsed() > ttl
    }

    /// Get the age of this response.
    pub fn age(&self) -> Duration {
        self.stored_at.elapsed()
    }
}

/// Configuration for the tool response store.
#[derive(Debug, Clone)]
pub struct ToolResponseStoreConfig {
    /// Maximum number of responses to store.
    pub max_size: usize,
    /// Time-to-live for stored responses.
    pub ttl: Duration,
    /// Whether to remove entries on read (peek vs consume).
    pub remove_on_read: bool,
}

impl Default for ToolResponseStoreConfig {
    fn default() -> Self {
        Self {
            max_size: MAX_STORE_SIZE,
            ttl: DEFAULT_TTL,
            remove_on_read: true,
        }
    }
}

impl ToolResponseStoreConfig {
    /// Create a config with custom max size.
    pub fn with_max_size(mut self, max_size: usize) -> Self {
        self.max_size = max_size;
        self
    }

    /// Create a config with custom TTL.
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Set whether to remove entries on read.
    pub fn with_remove_on_read(mut self, remove: bool) -> Self {
        self.remove_on_read = remove;
        self
    }
}

/// Bounded storage for tool execution responses.
///
/// This store prevents unbounded memory growth by:
/// 1. Enforcing a maximum number of stored responses
/// 2. Removing entries when they are consumed
/// 3. Periodically cleaning up stale entries
///
/// # Thread Safety
///
/// The store uses `RwLock` for interior mutability and is safe to share
/// across threads via `Arc<ToolResponseStore>`.
#[derive(Debug)]
pub struct ToolResponseStore {
    /// Stored responses keyed by tool call ID.
    responses: RwLock<HashMap<String, StoredResponse>>,
    /// Configuration.
    config: ToolResponseStoreConfig,
    /// Last cleanup time.
    last_cleanup: RwLock<Instant>,
    /// Statistics.
    stats: RwLock<StoreStats>,
}

impl ToolResponseStore {
    /// Create a new tool response store with default configuration.
    pub fn new() -> Self {
        Self::with_config(ToolResponseStoreConfig::default())
    }

    /// Create a tool response store with custom configuration.
    pub fn with_config(config: ToolResponseStoreConfig) -> Self {
        Self {
            responses: RwLock::new(HashMap::new()),
            config,
            last_cleanup: RwLock::new(Instant::now()),
            stats: RwLock::new(StoreStats::default()),
        }
    }

    /// Store a tool response.
    ///
    /// If the store is at capacity, the oldest entry will be evicted.
    /// Returns `true` if an entry was evicted to make room.
    pub async fn store(
        &self,
        call_id: impl Into<String>,
        tool_name: impl Into<String>,
        result: ToolResult,
    ) -> bool {
        let call_id = call_id.into();
        let tool_name = tool_name.into();
        let mut evicted = false;

        // Perform periodic cleanup if needed
        self.maybe_cleanup().await;

        let mut responses = self.responses.write().await;

        // Evict oldest entry if at capacity
        if responses.len() >= self.config.max_size {
            if let Some(oldest_key) = self.find_oldest_key(&responses) {
                responses.remove(&oldest_key);
                evicted = true;
                debug!(
                    evicted_key = %oldest_key,
                    "Evicted oldest response to make room"
                );
            }
        }

        let response = StoredResponse::new(tool_name, result);
        responses.insert(call_id.clone(), response);

        // Update stats
        let mut stats = self.stats.write().await;
        stats.total_stored += 1;
        if evicted {
            stats.evictions += 1;
        }

        evicted
    }

    /// Get a response without removing it (peek).
    ///
    /// Marks the response as read but does not consume it.
    pub async fn get(&self, call_id: &str) -> Option<ToolResult> {
        let mut responses = self.responses.write().await;

        if let Some(response) = responses.get_mut(call_id) {
            response.read = true;
            let mut stats = self.stats.write().await;
            stats.reads += 1;
            Some(response.result.clone())
        } else {
            None
        }
    }

    /// Take (consume) a response, removing it from the store.
    ///
    /// This is the primary method for retrieving responses as it ensures
    /// entries are cleaned up after being consumed (#5293).
    pub async fn take(&self, call_id: &str) -> Option<ToolResult> {
        let mut responses = self.responses.write().await;

        if let Some(response) = responses.remove(call_id) {
            let mut stats = self.stats.write().await;
            stats.takes += 1;
            Some(response.result)
        } else {
            None
        }
    }

    /// Check if a response exists for the given call ID.
    pub async fn contains(&self, call_id: &str) -> bool {
        self.responses.read().await.contains_key(call_id)
    }

    /// Get the current number of stored responses.
    pub async fn len(&self) -> usize {
        self.responses.read().await.len()
    }

    /// Check if the store is empty.
    pub async fn is_empty(&self) -> bool {
        self.responses.read().await.is_empty()
    }

    /// Remove all expired entries.
    ///
    /// Returns the number of entries removed.
    pub async fn cleanup_expired(&self) -> usize {
        let mut responses = self.responses.write().await;
        let ttl = self.config.ttl;
        let before = responses.len();

        responses.retain(|_, v| !v.is_expired(ttl));

        let removed = before - responses.len();
        if removed > 0 {
            debug!(removed, "Cleaned up expired responses");
            let mut stats = self.stats.write().await;
            stats.expired_cleanups += removed as u64;
        }

        removed
    }

    /// Remove all read entries that haven't been consumed.
    ///
    /// This is useful for cleaning up entries that were peeked but never taken.
    pub async fn cleanup_read(&self) -> usize {
        let mut responses = self.responses.write().await;
        let before = responses.len();

        responses.retain(|_, v| !v.read);

        let removed = before - responses.len();
        if removed > 0 {
            debug!(removed, "Cleaned up read-but-not-consumed responses");
        }

        removed
    }

    /// Clear all stored responses.
    pub async fn clear(&self) {
        self.responses.write().await.clear();
    }

    /// Get store statistics.
    pub async fn stats(&self) -> StoreStats {
        self.stats.read().await.clone()
    }

    /// Get detailed store info including current size and config.
    pub async fn info(&self) -> StoreInfo {
        let responses = self.responses.read().await;
        let stats = self.stats.read().await;

        StoreInfo {
            current_size: responses.len(),
            max_size: self.config.max_size,
            ttl_secs: self.config.ttl.as_secs(),
            oldest_age_secs: responses
                .values()
                .map(|r| r.age().as_secs())
                .max()
                .unwrap_or(0),
            stats: stats.clone(),
        }
    }

    // Internal helpers

    /// Find the key of the oldest entry.
    fn find_oldest_key(&self, responses: &HashMap<String, StoredResponse>) -> Option<String> {
        responses
            .iter()
            .min_by_key(|(_, v)| v.stored_at)
            .map(|(k, _)| k.clone())
    }

    /// Perform cleanup if enough time has passed since last cleanup.
    async fn maybe_cleanup(&self) {
        let should_cleanup = {
            let last = self.last_cleanup.read().await;
            last.elapsed() > CLEANUP_INTERVAL
        };

        if should_cleanup {
            *self.last_cleanup.write().await = Instant::now();
            let removed = self.cleanup_expired().await;
            if removed > 0 {
                debug!(removed, "Periodic cleanup removed expired entries");
            }
        }
    }
}

impl Default for ToolResponseStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics for the tool response store.
#[derive(Debug, Clone, Default)]
pub struct StoreStats {
    /// Total responses stored.
    pub total_stored: u64,
    /// Number of get (peek) operations.
    pub reads: u64,
    /// Number of take (consume) operations.
    pub takes: u64,
    /// Number of evictions due to capacity limit.
    pub evictions: u64,
    /// Number of entries removed by TTL cleanup.
    pub expired_cleanups: u64,
}

/// Detailed store information.
#[derive(Debug, Clone)]
pub struct StoreInfo {
    /// Current number of stored responses.
    pub current_size: usize,
    /// Maximum allowed size.
    pub max_size: usize,
    /// TTL in seconds.
    pub ttl_secs: u64,
    /// Age of oldest entry in seconds.
    pub oldest_age_secs: u64,
    /// Store statistics.
    pub stats: StoreStats,
}

/// Create a shared tool response store.
pub fn create_shared_store() -> Arc<ToolResponseStore> {
    Arc::new(ToolResponseStore::new())
}

/// Create a shared tool response store with custom configuration.
pub fn create_shared_store_with_config(config: ToolResponseStoreConfig) -> Arc<ToolResponseStore> {
    Arc::new(ToolResponseStore::with_config(config))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_store_and_take() {
        let store = ToolResponseStore::new();

        let result = ToolResult::success("test output");
        store.store("call-1", "Read", result.clone()).await;

        assert!(store.contains("call-1").await);
        assert_eq!(store.len().await, 1);

        let taken = store.take("call-1").await;
        assert!(taken.is_some());
        assert_eq!(taken.unwrap().output, "test output");

        // After take, entry should be gone
        assert!(!store.contains("call-1").await);
        assert_eq!(store.len().await, 0);
    }

    #[tokio::test]
    async fn test_store_and_get() {
        let store = ToolResponseStore::new();

        let result = ToolResult::success("test output");
        store.store("call-1", "Read", result).await;

        // Get should return result but not remove it
        let got = store.get("call-1").await;
        assert!(got.is_some());
        assert!(store.contains("call-1").await);

        // Second get should still work
        let got2 = store.get("call-1").await;
        assert!(got2.is_some());
    }

    #[tokio::test]
    async fn test_capacity_eviction() {
        let config = ToolResponseStoreConfig::default().with_max_size(3);
        let store = ToolResponseStore::with_config(config);

        // Fill to capacity
        store
            .store("call-1", "Read", ToolResult::success("1"))
            .await;
        store
            .store("call-2", "Read", ToolResult::success("2"))
            .await;
        store
            .store("call-3", "Read", ToolResult::success("3"))
            .await;

        assert_eq!(store.len().await, 3);

        // Add one more, should evict oldest
        let evicted = store
            .store("call-4", "Read", ToolResult::success("4"))
            .await;
        assert!(evicted);
        assert_eq!(store.len().await, 3);

        // call-1 should be evicted (oldest)
        assert!(!store.contains("call-1").await);
        assert!(store.contains("call-4").await);
    }

    #[tokio::test]
    async fn test_expired_cleanup() {
        let config = ToolResponseStoreConfig::default().with_ttl(Duration::from_millis(50));
        let store = ToolResponseStore::with_config(config);

        store
            .store("call-1", "Read", ToolResult::success("1"))
            .await;
        assert_eq!(store.len().await, 1);

        // Wait for expiration
        tokio::time::sleep(Duration::from_millis(100)).await;

        let removed = store.cleanup_expired().await;
        assert_eq!(removed, 1);
        assert_eq!(store.len().await, 0);
    }

    #[tokio::test]
    async fn test_cleanup_read() {
        let store = ToolResponseStore::new();

        store
            .store("call-1", "Read", ToolResult::success("1"))
            .await;
        store
            .store("call-2", "Read", ToolResult::success("2"))
            .await;

        // Read one entry
        store.get("call-1").await;

        // Cleanup read entries
        let removed = store.cleanup_read().await;
        assert_eq!(removed, 1);
        assert_eq!(store.len().await, 1);
        assert!(!store.contains("call-1").await);
        assert!(store.contains("call-2").await);
    }

    #[tokio::test]
    async fn test_stats() {
        let store = ToolResponseStore::new();

        store
            .store("call-1", "Read", ToolResult::success("1"))
            .await;
        store.get("call-1").await;
        store.take("call-1").await;

        let stats = store.stats().await;
        assert_eq!(stats.total_stored, 1);
        assert_eq!(stats.reads, 1);
        assert_eq!(stats.takes, 1);
    }

    #[tokio::test]
    async fn test_nonexistent_key() {
        let store = ToolResponseStore::new();

        assert!(store.get("nonexistent").await.is_none());
        assert!(store.take("nonexistent").await.is_none());
        assert!(!store.contains("nonexistent").await);
    }

    #[tokio::test]
    async fn test_clear() {
        let store = ToolResponseStore::new();

        store
            .store("call-1", "Read", ToolResult::success("1"))
            .await;
        store
            .store("call-2", "Read", ToolResult::success("2"))
            .await;

        assert_eq!(store.len().await, 2);

        store.clear().await;
        assert_eq!(store.len().await, 0);
    }

    #[tokio::test]
    async fn test_info() {
        let store = ToolResponseStore::new();

        store
            .store("call-1", "Read", ToolResult::success("1"))
            .await;

        let info = store.info().await;
        assert_eq!(info.current_size, 1);
        assert_eq!(info.max_size, MAX_STORE_SIZE);
    }
}
