//! Rate limiting and throttling.
//!
//! Provides rate limiting functionality for API calls, tool executions,
//! and other operations that need to be throttled.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock, Semaphore};
use tokio::time::sleep;

use crate::error::{CortexError, Result};

/// Rate limiter configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Requests per second.
    pub requests_per_second: f64,
    /// Burst size (maximum concurrent requests).
    pub burst_size: u32,
    /// Maximum wait time for acquiring a permit.
    pub max_wait: Duration,
    /// Enable adaptive rate limiting.
    pub adaptive: bool,
    /// Minimum rate when adapting.
    pub min_rate: f64,
    /// Maximum rate when adapting.
    pub max_rate: f64,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            requests_per_second: 10.0,
            burst_size: 20,
            max_wait: Duration::from_secs(30),
            adaptive: false,
            min_rate: 1.0,
            max_rate: 100.0,
        }
    }
}

/// Token bucket rate limiter.
#[derive(Debug)]
pub struct TokenBucket {
    /// Configuration.
    config: RateLimitConfig,
    /// Current tokens.
    tokens: Mutex<f64>,
    /// Last refill time.
    last_refill: Mutex<Instant>,
    /// Current rate (for adaptive limiting).
    current_rate: Mutex<f64>,
}

impl TokenBucket {
    /// Create a new token bucket.
    pub fn new(config: RateLimitConfig) -> Self {
        let rate = config.requests_per_second;
        // Initialize with a full burst of tokens
        Self {
            config: config.clone(),
            tokens: Mutex::new(config.burst_size as f64),
            last_refill: Mutex::new(Instant::now()),
            current_rate: Mutex::new(rate),
        }
    }

    /// Create with default config.
    pub fn default_limiter() -> Self {
        Self::new(RateLimitConfig::default())
    }

    /// Try to acquire a permit (non-blocking).
    pub async fn try_acquire(&self) -> bool {
        self.refill().await;

        let mut tokens = self.tokens.lock().await;
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Acquire a permit (blocking with timeout).
    pub async fn acquire(&self) -> Result<()> {
        let deadline = Instant::now() + self.config.max_wait;

        loop {
            if self.try_acquire().await {
                return Ok(());
            }

            if Instant::now() >= deadline {
                return Err(CortexError::RateLimit("Rate limit exceeded".to_string()));
            }

            // Calculate wait time
            let rate = *self.current_rate.lock().await;
            let wait_time = Duration::from_secs_f64(1.0 / rate);
            sleep(wait_time.min(Duration::from_millis(100))).await;
        }
    }

    /// Acquire multiple permits.
    pub async fn acquire_n(&self, n: u32) -> Result<()> {
        for _ in 0..n {
            self.acquire().await?;
        }
        Ok(())
    }

    /// Refill tokens based on elapsed time.
    async fn refill(&self) {
        let mut last = self.last_refill.lock().await;
        let mut tokens = self.tokens.lock().await;
        let rate = *self.current_rate.lock().await;

        let now = Instant::now();
        let elapsed = now.duration_since(*last).as_secs_f64();
        let new_tokens = elapsed * rate;

        *tokens = (*tokens + new_tokens).min(self.config.burst_size as f64);
        *last = now;
    }

    /// Adjust rate (for adaptive limiting).
    pub async fn adjust_rate(&self, factor: f64) {
        if !self.config.adaptive {
            return;
        }

        let mut rate = self.current_rate.lock().await;
        let new_rate = (*rate * factor)
            .max(self.config.min_rate)
            .min(self.config.max_rate);
        *rate = new_rate;
    }

    /// Get current rate.
    pub async fn current_rate(&self) -> f64 {
        *self.current_rate.lock().await
    }

    /// Get available tokens.
    pub async fn available_tokens(&self) -> f64 {
        self.refill().await;
        *self.tokens.lock().await
    }
}

/// Sliding window rate limiter.
#[derive(Debug)]
pub struct SlidingWindow {
    /// Window duration.
    window: Duration,
    /// Maximum requests per window.
    max_requests: u32,
    /// Request timestamps.
    requests: Mutex<Vec<Instant>>,
}

impl SlidingWindow {
    /// Create a new sliding window limiter.
    pub fn new(window: Duration, max_requests: u32) -> Self {
        Self {
            window,
            max_requests,
            requests: Mutex::new(Vec::new()),
        }
    }

    /// Create with requests per minute.
    pub fn per_minute(max_requests: u32) -> Self {
        Self::new(Duration::from_secs(60), max_requests)
    }

    /// Create with requests per second.
    pub fn per_second(max_requests: u32) -> Self {
        Self::new(Duration::from_secs(1), max_requests)
    }

    /// Check if request is allowed.
    pub async fn is_allowed(&self) -> bool {
        self.cleanup().await;

        let requests = self.requests.lock().await;
        requests.len() < self.max_requests as usize
    }

    /// Try to acquire a permit.
    pub async fn try_acquire(&self) -> bool {
        self.cleanup().await;

        let mut requests = self.requests.lock().await;
        if requests.len() < self.max_requests as usize {
            requests.push(Instant::now());
            true
        } else {
            false
        }
    }

    /// Acquire a permit (blocking).
    pub async fn acquire(&self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;

        loop {
            if self.try_acquire().await {
                return Ok(());
            }

            if Instant::now() >= deadline {
                return Err(CortexError::RateLimit("Rate limit exceeded".to_string()));
            }

            // Wait until oldest request expires
            let wait_time = {
                let requests = self.requests.lock().await;
                if let Some(oldest) = requests.first() {
                    let age = oldest.elapsed();
                    if age < self.window {
                        self.window - age
                    } else {
                        Duration::from_millis(10)
                    }
                } else {
                    Duration::from_millis(10)
                }
            };

            sleep(wait_time.min(Duration::from_millis(100))).await;
        }
    }

    /// Get current request count.
    pub async fn current_count(&self) -> usize {
        self.cleanup().await;
        self.requests.lock().await.len()
    }

    /// Get remaining requests.
    pub async fn remaining(&self) -> u32 {
        self.cleanup().await;
        let count = self.requests.lock().await.len();
        self.max_requests.saturating_sub(count as u32)
    }

    /// Cleanup expired requests.
    async fn cleanup(&self) {
        let mut requests = self.requests.lock().await;
        let cutoff = Instant::now() - self.window;
        requests.retain(|t| *t > cutoff);
    }

    /// Reset the limiter.
    pub async fn reset(&self) {
        self.requests.lock().await.clear();
    }
}

/// Fixed window rate limiter.
#[derive(Debug)]
pub struct FixedWindow {
    /// Window duration.
    window: Duration,
    /// Maximum requests per window.
    max_requests: u32,
    /// Current window start.
    window_start: Mutex<Instant>,
    /// Current window count.
    count: Mutex<u32>,
}

impl FixedWindow {
    /// Create a new fixed window limiter.
    pub fn new(window: Duration, max_requests: u32) -> Self {
        Self {
            window,
            max_requests,
            window_start: Mutex::new(Instant::now()),
            count: Mutex::new(0),
        }
    }

    /// Try to acquire a permit.
    pub async fn try_acquire(&self) -> bool {
        self.maybe_reset().await;

        let mut count = self.count.lock().await;
        if *count < self.max_requests {
            *count += 1;
            true
        } else {
            false
        }
    }

    /// Check if we need to reset the window.
    async fn maybe_reset(&self) {
        let mut start = self.window_start.lock().await;
        if start.elapsed() >= self.window {
            *start = Instant::now();
            *self.count.lock().await = 0;
        }
    }

    /// Get time until window reset.
    pub async fn time_until_reset(&self) -> Duration {
        let start = self.window_start.lock().await;
        let elapsed = start.elapsed();
        if elapsed >= self.window {
            Duration::ZERO
        } else {
            self.window - elapsed
        }
    }
}

/// Concurrent request limiter (semaphore-based).
#[derive(Debug)]
pub struct ConcurrencyLimiter {
    /// Semaphore.
    semaphore: Arc<Semaphore>,
    /// Maximum concurrent.
    max_concurrent: usize,
}

impl ConcurrencyLimiter {
    /// Create a new concurrency limiter.
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            max_concurrent,
        }
    }

    /// Acquire a permit.
    pub async fn acquire(&self) -> ConcurrencyPermit {
        let permit = self.semaphore.clone().acquire_owned().await.unwrap();
        ConcurrencyPermit { _permit: permit }
    }

    /// Try to acquire a permit.
    pub fn try_acquire(&self) -> Option<ConcurrencyPermit> {
        self.semaphore
            .clone()
            .try_acquire_owned()
            .ok()
            .map(|permit| ConcurrencyPermit { _permit: permit })
    }

    /// Get available permits.
    pub fn available(&self) -> usize {
        self.semaphore.available_permits()
    }

    /// Get active count.
    pub fn active(&self) -> usize {
        self.max_concurrent - self.available()
    }
}

/// Concurrency permit (RAII guard).
pub struct ConcurrencyPermit {
    _permit: tokio::sync::OwnedSemaphorePermit,
}

/// Multi-key token bucket rate limiter.
pub struct KeyedTokenBucketLimiter {
    /// Per-key limiters.
    limiters: RwLock<HashMap<String, Arc<TokenBucket>>>,
    /// Config for new limiters.
    config: RateLimitConfig,
}

impl KeyedTokenBucketLimiter {
    /// Create a new keyed rate limiter.
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            limiters: RwLock::new(HashMap::new()),
            config,
        }
    }

    /// Get or create limiter for key.
    pub async fn get(&self, key: &str) -> Arc<TokenBucket> {
        // Try read first
        {
            let limiters = self.limiters.read().await;
            if let Some(limiter) = limiters.get(key) {
                return limiter.clone();
            }
        }

        // Create new limiter
        let mut limiters = self.limiters.write().await;
        let limiter = Arc::new(TokenBucket::new(self.config.clone()));
        limiters.insert(key.to_string(), limiter.clone());
        limiter
    }

    /// Remove limiter for key.
    pub async fn remove(&self, key: &str) {
        self.limiters.write().await.remove(key);
    }

    /// Clear all limiters.
    pub async fn clear(&self) {
        self.limiters.write().await.clear();
    }

    /// Get number of keys.
    pub async fn len(&self) -> usize {
        self.limiters.read().await.len()
    }

    /// Check if empty.
    pub async fn is_empty(&self) -> bool {
        self.limiters.read().await.is_empty()
    }
}

/// Composite rate limiter using token buckets.
pub struct CompositeRateLimiter {
    /// Token bucket limiters.
    token_buckets: Vec<TokenBucket>,
    /// Sliding window limiters.
    sliding_windows: Vec<SlidingWindow>,
}

impl CompositeRateLimiter {
    /// Create a new composite limiter.
    pub fn new() -> Self {
        Self {
            token_buckets: Vec::new(),
            sliding_windows: Vec::new(),
        }
    }

    /// Add a token bucket limiter.
    pub fn add_token_bucket(mut self, limiter: TokenBucket) -> Self {
        self.token_buckets.push(limiter);
        self
    }

    /// Add a sliding window limiter.
    pub fn add_sliding_window(mut self, limiter: SlidingWindow) -> Self {
        self.sliding_windows.push(limiter);
        self
    }

    /// Try to acquire from all limiters.
    pub async fn try_acquire(&self) -> bool {
        for limiter in &self.token_buckets {
            if !limiter.try_acquire().await {
                return false;
            }
        }
        for limiter in &self.sliding_windows {
            if !limiter.try_acquire().await {
                return false;
            }
        }
        true
    }
}

impl Default for CompositeRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

/// Rate limit status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitStatus {
    /// Whether requests are allowed.
    pub allowed: bool,
    /// Remaining requests.
    pub remaining: u32,
    /// Time until reset (seconds).
    pub reset_after: f64,
    /// Current rate.
    pub current_rate: f64,
}

/// Builder for rate limiters.
pub struct RateLimiterBuilder {
    config: RateLimitConfig,
}

impl RateLimiterBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            config: RateLimitConfig::default(),
        }
    }

    /// Set requests per second.
    pub fn rate(mut self, rps: f64) -> Self {
        self.config.requests_per_second = rps;
        self
    }

    /// Set burst size.
    pub fn burst(mut self, size: u32) -> Self {
        self.config.burst_size = size;
        self
    }

    /// Set max wait time.
    pub fn max_wait(mut self, duration: Duration) -> Self {
        self.config.max_wait = duration;
        self
    }

    /// Enable adaptive rate limiting.
    pub fn adaptive(mut self, min_rate: f64, max_rate: f64) -> Self {
        self.config.adaptive = true;
        self.config.min_rate = min_rate;
        self.config.max_rate = max_rate;
        self
    }

    /// Build token bucket limiter.
    pub fn build_token_bucket(self) -> TokenBucket {
        TokenBucket::new(self.config)
    }

    /// Build sliding window limiter.
    pub fn build_sliding_window(self, window: Duration) -> SlidingWindow {
        SlidingWindow::new(window, self.config.burst_size)
    }

    /// Build fixed window limiter.
    pub fn build_fixed_window(self, window: Duration) -> FixedWindow {
        FixedWindow::new(window, self.config.burst_size)
    }
}

impl Default for RateLimiterBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_token_bucket() {
        let limiter = TokenBucket::new(RateLimitConfig {
            requests_per_second: 10.0,
            burst_size: 5,
            ..Default::default()
        });

        // Should allow burst
        for _ in 0..5 {
            assert!(limiter.try_acquire().await);
        }

        // Should be rate limited
        assert!(!limiter.try_acquire().await);
    }

    #[tokio::test]
    async fn test_sliding_window() {
        let limiter = SlidingWindow::new(Duration::from_millis(100), 5);

        // Should allow up to max
        for _ in 0..5 {
            assert!(limiter.try_acquire().await);
        }

        // Should be rate limited
        assert!(!limiter.try_acquire().await);

        // Wait for window to slide
        sleep(Duration::from_millis(150)).await;

        // Should allow again
        assert!(limiter.try_acquire().await);
    }

    #[tokio::test]
    async fn test_concurrency_limiter() {
        let limiter = ConcurrencyLimiter::new(2);

        let _p1 = limiter.acquire().await;
        let _p2 = limiter.acquire().await;

        // Third should fail immediately
        assert!(limiter.try_acquire().is_none());

        assert_eq!(limiter.active(), 2);
        assert_eq!(limiter.available(), 0);
    }

    #[tokio::test]
    async fn test_fixed_window() {
        let limiter = FixedWindow::new(Duration::from_millis(100), 3);

        assert!(limiter.try_acquire().await);
        assert!(limiter.try_acquire().await);
        assert!(limiter.try_acquire().await);
        assert!(!limiter.try_acquire().await);
    }

    #[tokio::test]
    async fn test_builder() {
        let limiter = RateLimiterBuilder::new()
            .rate(5.0)
            .burst(10)
            .build_token_bucket();

        assert_eq!(limiter.current_rate().await, 5.0);
    }
}
