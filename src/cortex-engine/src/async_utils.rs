//! Async utilities.
//!
//! Provides utilities for async operations including
//! timeouts, retries, and concurrency control.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, RwLock, Semaphore};

use crate::error::{CortexError, Result};

/// Async timeout wrapper.
pub async fn timeout<F, T>(duration: Duration, fut: F) -> Result<T>
where
    F: Future<Output = T>,
{
    tokio::time::timeout(duration, fut)
        .await
        .map_err(|_| CortexError::Timeout)
}

/// Async retry with exponential backoff.
pub async fn retry<F, Fut, T, E>(
    max_attempts: u32,
    initial_delay: Duration,
    max_delay: Duration,
    operation: F,
) -> std::result::Result<T, E>
where
    F: Fn() -> Fut,
    Fut: Future<Output = std::result::Result<T, E>>,
    E: std::fmt::Debug,
{
    let mut attempt = 0;
    let mut delay = initial_delay;

    loop {
        attempt += 1;

        match operation().await {
            Ok(result) => return Ok(result),
            Err(e) if attempt >= max_attempts => return Err(e),
            Err(_) => {
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(max_delay);
            }
        }
    }
}

/// Async debounce.
pub struct Debounce {
    delay: Duration,
    last_call: Mutex<Option<Instant>>,
}

impl Debounce {
    /// Create a new debounce.
    pub fn new(delay: Duration) -> Self {
        Self {
            delay,
            last_call: Mutex::new(None),
        }
    }

    /// Check if should execute.
    pub async fn should_execute(&self) -> bool {
        let mut last = self.last_call.lock().await;
        let now = Instant::now();

        if let Some(last_time) = *last
            && now.duration_since(last_time) < self.delay
        {
            return false;
        }

        *last = Some(now);
        true
    }

    /// Execute if not debounced.
    pub async fn execute<F, Fut, T>(&self, f: F) -> Option<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        if self.should_execute().await {
            Some(f().await)
        } else {
            None
        }
    }
}

/// Async throttle.
pub struct Throttle {
    interval: Duration,
    last_execution: Mutex<Option<Instant>>,
}

impl Throttle {
    /// Create a new throttle.
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_execution: Mutex::new(None),
        }
    }

    /// Wait and execute.
    pub async fn execute<F, Fut, T>(&self, f: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        let mut last = self.last_execution.lock().await;
        let now = Instant::now();

        if let Some(last_time) = *last {
            let elapsed = now.duration_since(last_time);
            if elapsed < self.interval {
                tokio::time::sleep(self.interval - elapsed).await;
            }
        }

        *last = Some(Instant::now());
        drop(last);

        f().await
    }
}

/// Concurrent limiter.
pub struct ConcurrencyLimiter {
    semaphore: Arc<Semaphore>,
}

impl ConcurrencyLimiter {
    /// Create a new limiter.
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
        }
    }

    /// Execute with limit.
    ///
    /// Returns an error if the semaphore is closed.
    pub async fn execute<F, Fut, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        let _permit = self.semaphore.acquire().await.map_err(|_| {
            CortexError::Internal("concurrency limiter semaphore closed unexpectedly".into())
        })?;
        Ok(f().await)
    }

    /// Get available permits.
    pub fn available(&self) -> usize {
        self.semaphore.available_permits()
    }
}

/// Async once cell.
pub struct AsyncOnce<T> {
    value: RwLock<Option<T>>,
    initialized: RwLock<bool>,
}

impl<T: Clone> AsyncOnce<T> {
    /// Create a new once cell.
    pub fn new() -> Self {
        Self {
            value: RwLock::new(None),
            initialized: RwLock::new(false),
        }
    }

    /// Get or initialize.
    ///
    /// Returns an error if the internal state is inconsistent (value missing after init flag set).
    pub async fn get_or_init<F, Fut>(&self, init: F) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        // Fast path
        if *self.initialized.read().await {
            return self.value.read().await.clone().ok_or_else(|| {
                CortexError::Internal(
                    "AsyncOnce: value missing despite initialized flag being set".into(),
                )
            });
        }

        // Slow path
        let mut initialized = self.initialized.write().await;
        if *initialized {
            return self.value.read().await.clone().ok_or_else(|| {
                CortexError::Internal(
                    "AsyncOnce: value missing despite initialized flag being set".into(),
                )
            });
        }

        let value = init().await;
        *self.value.write().await = Some(value.clone());
        *initialized = true;
        Ok(value)
    }

    /// Check if initialized.
    pub async fn is_initialized(&self) -> bool {
        *self.initialized.read().await
    }
}

impl<T: Clone> Default for AsyncOnce<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Async queue.
pub struct AsyncQueue<T> {
    items: RwLock<Vec<T>>,
    capacity: Option<usize>,
}

impl<T> AsyncQueue<T> {
    /// Create a new queue.
    pub fn new() -> Self {
        Self {
            items: RwLock::new(Vec::new()),
            capacity: None,
        }
    }

    /// Create with capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            items: RwLock::new(Vec::with_capacity(capacity)),
            capacity: Some(capacity),
        }
    }

    /// Push an item.
    pub async fn push(&self, item: T) -> bool {
        let mut items = self.items.write().await;

        if let Some(cap) = self.capacity
            && items.len() >= cap
        {
            return false;
        }

        items.push(item);
        true
    }

    /// Pop an item.
    pub async fn pop(&self) -> Option<T> {
        let mut items = self.items.write().await;
        if items.is_empty() {
            None
        } else {
            Some(items.remove(0))
        }
    }

    /// Get length.
    pub async fn len(&self) -> usize {
        self.items.read().await.len()
    }

    /// Check if empty.
    pub async fn is_empty(&self) -> bool {
        self.items.read().await.is_empty()
    }

    /// Clear the queue.
    pub async fn clear(&self) {
        self.items.write().await.clear();
    }
}

impl<T> Default for AsyncQueue<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Async batch processor.
pub struct BatchProcessor<T> {
    batch_size: usize,
    batch_timeout: Duration,
    items: Mutex<Vec<T>>,
    last_flush: Mutex<Instant>,
}

impl<T> BatchProcessor<T> {
    /// Create a new processor.
    pub fn new(batch_size: usize, batch_timeout: Duration) -> Self {
        Self {
            batch_size,
            batch_timeout,
            items: Mutex::new(Vec::with_capacity(batch_size)),
            last_flush: Mutex::new(Instant::now()),
        }
    }

    /// Add an item.
    pub async fn add(&self, item: T) -> Option<Vec<T>> {
        let mut items = self.items.lock().await;
        items.push(item);

        if items.len() >= self.batch_size {
            let batch = std::mem::take(&mut *items);
            *self.last_flush.lock().await = Instant::now();
            return Some(batch);
        }

        // Check timeout
        let last = *self.last_flush.lock().await;
        if last.elapsed() >= self.batch_timeout && !items.is_empty() {
            let batch = std::mem::take(&mut *items);
            *self.last_flush.lock().await = Instant::now();
            return Some(batch);
        }

        None
    }

    /// Flush remaining items.
    pub async fn flush(&self) -> Vec<T> {
        let mut items = self.items.lock().await;
        *self.last_flush.lock().await = Instant::now();
        std::mem::take(&mut *items)
    }
}

/// Async cache with TTL.
pub struct AsyncCache<K, V> {
    entries: RwLock<std::collections::HashMap<K, CacheEntry<V>>>,
    ttl: Duration,
}

struct CacheEntry<V> {
    value: V,
    expires_at: Instant,
}

impl<K: std::hash::Hash + Eq + Clone, V: Clone> AsyncCache<K, V> {
    /// Create a new cache.
    pub fn new(ttl: Duration) -> Self {
        Self {
            entries: RwLock::new(std::collections::HashMap::new()),
            ttl,
        }
    }

    /// Get a value.
    pub async fn get(&self, key: &K) -> Option<V> {
        let entries = self.entries.read().await;
        if let Some(entry) = entries.get(key)
            && Instant::now() < entry.expires_at
        {
            return Some(entry.value.clone());
        }
        None
    }

    /// Set a value.
    pub async fn set(&self, key: K, value: V) {
        let expires_at = Instant::now() + self.ttl;
        self.entries
            .write()
            .await
            .insert(key, CacheEntry { value, expires_at });
    }

    /// Get or set.
    pub async fn get_or_set<F, Fut>(&self, key: K, f: F) -> V
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = V>,
    {
        if let Some(value) = self.get(&key).await {
            return value;
        }

        let value = f().await;
        self.set(key, value.clone()).await;
        value
    }

    /// Remove expired entries.
    pub async fn cleanup(&self) {
        let now = Instant::now();
        let mut entries = self.entries.write().await;
        entries.retain(|_, entry| entry.expires_at > now);
    }

    /// Clear all entries.
    pub async fn clear(&self) {
        self.entries.write().await.clear();
    }
}

/// Run futures concurrently with limit.
///
/// Returns an error if the semaphore is closed unexpectedly.
pub async fn concurrent<F, Fut, T>(
    items: impl IntoIterator<Item = F>,
    limit: usize,
) -> Result<Vec<T>>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = T>,
{
    let semaphore = Arc::new(Semaphore::new(limit));
    let mut handles = Vec::new();

    for item in items {
        let sem = semaphore.clone();
        handles.push(async move {
            let _permit = sem.acquire().await.map_err(|_| {
                CortexError::Internal("concurrent execution semaphore closed unexpectedly".into())
            })?;
            Ok(item().await)
        });
    }

    futures::future::join_all(handles)
        .await
        .into_iter()
        .collect()
}

/// Select the first future to complete.
pub async fn race<T>(futures: Vec<Pin<Box<dyn Future<Output = T> + Send>>>) -> Option<T> {
    if futures.is_empty() {
        return None;
    }

    use futures::future::select_all;
    let (result, _, _) = select_all(futures).await;
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_timeout_success() {
        let result = timeout(Duration::from_secs(1), async { 42 }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_timeout_failure() {
        let result = timeout(Duration::from_millis(10), async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            42
        })
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_retry() {
        let counter = Arc::new(Mutex::new(0));
        let counter_clone = counter.clone();

        let result = retry(
            3,
            Duration::from_millis(10),
            Duration::from_millis(100),
            || {
                let c = counter_clone.clone();
                async move {
                    let mut count = c.lock().await;
                    *count += 1;
                    if *count < 3 {
                        Err("not yet")
                    } else {
                        Ok(*count)
                    }
                }
            },
        )
        .await;

        assert_eq!(result.unwrap(), 3);
    }

    #[tokio::test]
    async fn test_debounce() {
        let debounce = Debounce::new(Duration::from_millis(50));

        assert!(debounce.should_execute().await);
        assert!(!debounce.should_execute().await);

        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(debounce.should_execute().await);
    }

    #[tokio::test]
    async fn test_concurrency_limiter() {
        let limiter = ConcurrencyLimiter::new(2);

        let counter = Arc::new(Mutex::new(0));
        let mut handles = Vec::new();

        for _ in 0..5 {
            let l = &limiter;
            let c = counter.clone();
            handles.push(l.execute(|| async move {
                let mut count = c.lock().await;
                *count += 1;
            }));
        }

        let results: Vec<_> = futures::future::join_all(handles).await;
        for result in results {
            assert!(result.is_ok());
        }
        assert_eq!(*counter.lock().await, 5);
    }

    #[tokio::test]
    async fn test_async_once() {
        let once: AsyncOnce<i32> = AsyncOnce::new();

        let v1 = once.get_or_init(|| async { 42 }).await.unwrap();
        let v2 = once.get_or_init(|| async { 100 }).await.unwrap();

        assert_eq!(v1, 42);
        assert_eq!(v2, 42);
    }

    #[tokio::test]
    async fn test_async_queue() {
        let queue: AsyncQueue<i32> = AsyncQueue::new();

        queue.push(1).await;
        queue.push(2).await;

        assert_eq!(queue.len().await, 2);
        assert_eq!(queue.pop().await, Some(1));
        assert_eq!(queue.pop().await, Some(2));
        assert!(queue.is_empty().await);
    }

    #[tokio::test]
    async fn test_batch_processor() {
        let processor: BatchProcessor<i32> = BatchProcessor::new(3, Duration::from_secs(10));

        assert!(processor.add(1).await.is_none());
        assert!(processor.add(2).await.is_none());

        let batch = processor.add(3).await;
        assert_eq!(batch, Some(vec![1, 2, 3]));
    }

    #[tokio::test]
    async fn test_async_cache() {
        let cache: AsyncCache<&str, i32> = AsyncCache::new(Duration::from_secs(10));

        cache.set("key", 42).await;
        assert_eq!(cache.get(&"key").await, Some(42));
        assert_eq!(cache.get(&"missing").await, None);
    }

    #[tokio::test]
    async fn test_concurrent() {
        let items: Vec<
            Box<dyn FnOnce() -> std::pin::Pin<Box<dyn Future<Output = i32> + Send>> + Send>,
        > = vec![
            Box::new(|| Box::pin(async { 1 })),
            Box::new(|| Box::pin(async { 2 })),
            Box::new(|| Box::pin(async { 3 })),
        ];
        let results = concurrent(items, 2).await.unwrap();

        assert_eq!(results.len(), 3);
    }
}
