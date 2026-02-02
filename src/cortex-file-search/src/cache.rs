//! Caching system for file listings with TTL support.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use crate::config::CacheConfig;

/// Entry in the file cache.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    /// List of file paths in this directory.
    pub files: Vec<PathBuf>,

    /// When this entry was last updated.
    pub updated_at: Instant,

    /// File system modification time when cached.
    pub fs_mtime: Option<u64>,
}

impl CacheEntry {
    /// Creates a new cache entry.
    pub fn new(files: Vec<PathBuf>, fs_mtime: Option<u64>) -> Self {
        Self {
            files,
            updated_at: Instant::now(),
            fs_mtime,
        }
    }

    /// Checks if this entry has expired based on TTL.
    pub fn is_expired(&self, ttl: Duration) -> bool {
        self.updated_at.elapsed() > ttl
    }
}

/// Cache for file listings with LRU eviction and TTL support.
#[derive(Debug)]
pub struct FileCache {
    /// Cached directory listings.
    entries: HashMap<PathBuf, CacheEntry>,

    /// Order of access for LRU eviction.
    access_order: Vec<PathBuf>,

    /// Configuration for this cache.
    config: CacheConfig,

    /// Statistics: cache hits.
    hits: AtomicU64,

    /// Statistics: cache misses.
    misses: AtomicU64,
}

impl FileCache {
    /// Creates a new file cache with the given configuration.
    pub fn new(config: CacheConfig) -> Self {
        Self {
            entries: HashMap::with_capacity(config.max_entries.min(1000)),
            access_order: Vec::with_capacity(config.max_entries.min(1000)),
            config,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    /// Creates a new file cache with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(CacheConfig::default())
    }

    /// Returns whether caching is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Gets a cached entry if it exists and is not expired.
    pub fn get(&mut self, path: &Path) -> Option<&CacheEntry> {
        if !self.config.enabled {
            return None;
        }

        let path_buf = path.to_path_buf();

        if let Some(entry) = self.entries.get(&path_buf) {
            // Check TTL
            if let Some(ttl_secs) = self.config.ttl_seconds
                && entry.is_expired(Duration::from_secs(ttl_secs))
            {
                self.misses.fetch_add(1, Ordering::Relaxed);
                return None;
            }

            // Update access order for LRU
            self.access_order.retain(|p| p != &path_buf);
            self.access_order.push(path_buf.clone());

            self.hits.fetch_add(1, Ordering::Relaxed);
            return self.entries.get(&path_buf);
        }

        self.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Inserts or updates a cache entry.
    pub fn insert(&mut self, path: PathBuf, files: Vec<PathBuf>, fs_mtime: Option<u64>) {
        if !self.config.enabled {
            return;
        }

        // Evict oldest entries if at capacity
        while self.entries.len() >= self.config.max_entries {
            if let Some(oldest) = self.access_order.first().cloned() {
                self.entries.remove(&oldest);
                self.access_order.remove(0);
            } else {
                break;
            }
        }

        // Update access order
        self.access_order.retain(|p| p != &path);
        self.access_order.push(path.clone());

        // Insert the entry
        self.entries.insert(path, CacheEntry::new(files, fs_mtime));
    }

    /// Invalidates a specific cache entry.
    pub fn invalidate(&mut self, path: &Path) {
        let path_buf = path.to_path_buf();
        self.entries.remove(&path_buf);
        self.access_order.retain(|p| p != &path_buf);
    }

    /// Invalidates all entries under a given path.
    pub fn invalidate_under(&mut self, path: &Path) {
        let paths_to_remove: Vec<PathBuf> = self
            .entries
            .keys()
            .filter(|p| p.starts_with(path))
            .cloned()
            .collect();

        for p in paths_to_remove {
            self.entries.remove(&p);
            self.access_order.retain(|ap| ap != &p);
        }
    }

    /// Clears all cached entries.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.access_order.clear();
    }

    /// Removes all expired entries.
    pub fn evict_expired(&mut self) {
        if let Some(ttl_secs) = self.config.ttl_seconds {
            let ttl = Duration::from_secs(ttl_secs);
            let expired: Vec<PathBuf> = self
                .entries
                .iter()
                .filter(|(_, entry)| entry.is_expired(ttl))
                .map(|(path, _)| path.clone())
                .collect();

            for path in expired {
                self.entries.remove(&path);
                self.access_order.retain(|p| p != &path);
            }
        }
    }

    /// Returns the number of cached entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns cache statistics.
    pub fn stats(&self) -> CacheStats {
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);

        CacheStats {
            entries: self.entries.len(),
            hits,
            misses,
            hit_rate: if hits + misses > 0 {
                hits as f64 / (hits + misses) as f64
            } else {
                0.0
            },
        }
    }

    /// Checks if a cached entry is still valid by comparing modification times.
    pub fn is_valid(&self, path: &Path, current_mtime: Option<u64>) -> bool {
        if let Some(entry) = self.entries.get(&path.to_path_buf()) {
            match (entry.fs_mtime, current_mtime) {
                (Some(cached), Some(current)) => cached == current,
                (None, None) => true,
                _ => false,
            }
        } else {
            false
        }
    }
}

/// Cache statistics.
#[derive(Debug, Clone)]
pub struct CacheStats {
    /// Number of entries in the cache.
    pub entries: usize,

    /// Number of cache hits.
    pub hits: u64,

    /// Number of cache misses.
    pub misses: u64,

    /// Cache hit rate (0.0 to 1.0).
    pub hit_rate: f64,
}

/// Gets the modification time of a path as a Unix timestamp.
pub fn get_mtime(path: &Path) -> Option<u64> {
    path.metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_basic_operations() {
        let mut cache = FileCache::with_defaults();

        let path = PathBuf::from("/test/dir");
        let files = vec![PathBuf::from("file1.rs"), PathBuf::from("file2.rs")];

        cache.insert(path.clone(), files.clone(), Some(12345));

        let entry = cache.get(&path);
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().files.len(), 2);
    }

    #[test]
    fn test_cache_lru_eviction() {
        let config = CacheConfig {
            enabled: true,
            max_entries: 3,
            ttl_seconds: None,
            persist_to_disk: false,
            cache_path: None,
        };

        let mut cache = FileCache::new(config);

        // Insert 3 entries
        for i in 0..3 {
            cache.insert(
                PathBuf::from(format!("/dir{i}")),
                vec![PathBuf::from(format!("file{i}.rs"))],
                None,
            );
        }

        assert_eq!(cache.len(), 3);

        // Access first entry to make it most recently used
        let _ = cache.get(Path::new("/dir0"));

        // Insert 4th entry, should evict /dir1 (oldest not recently accessed)
        cache.insert(
            PathBuf::from("/dir3"),
            vec![PathBuf::from("file3.rs")],
            None,
        );

        assert_eq!(cache.len(), 3);
        assert!(cache.entries.contains_key(Path::new("/dir0")));
        assert!(cache.entries.contains_key(Path::new("/dir3")));
    }

    #[test]
    fn test_cache_invalidation() {
        let mut cache = FileCache::with_defaults();

        cache.insert(PathBuf::from("/root/a"), vec![], None);
        cache.insert(PathBuf::from("/root/a/b"), vec![], None);
        cache.insert(PathBuf::from("/root/c"), vec![], None);

        assert_eq!(cache.len(), 3);

        cache.invalidate_under(Path::new("/root/a"));

        assert_eq!(cache.len(), 1);
        assert!(cache.entries.contains_key(Path::new("/root/c")));
    }

    #[test]
    fn test_cache_disabled() {
        let config = CacheConfig {
            enabled: false,
            ..Default::default()
        };

        let mut cache = FileCache::new(config);

        cache.insert(PathBuf::from("/test"), vec![], None);
        assert!(cache.get(Path::new("/test")).is_none());
        assert!(cache.is_empty());
    }

    #[test]
    fn test_cache_stats() {
        let mut cache = FileCache::with_defaults();

        cache.insert(PathBuf::from("/test"), vec![], None);

        // Hit
        let _ = cache.get(Path::new("/test"));
        // Miss
        let _ = cache.get(Path::new("/nonexistent"));

        let stats = cache.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
        assert!((stats.hit_rate - 0.5).abs() < 0.001);
    }
}
