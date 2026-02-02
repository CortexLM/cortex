//! Retry and backoff strategies.
//!
//! Provides configurable retry logic with various backoff strategies
//! for handling transient failures in API calls and operations.

use std::future::Future;
use std::time::Duration;

use rand::Rng;
use serde::{Deserialize, Serialize};
use tokio::time::sleep;
use tracing::{debug, warn};

use crate::error::{CortexError, Result};

/// Retry configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    /// Maximum number of attempts.
    pub max_attempts: u32,
    /// Initial delay.
    pub initial_delay: Duration,
    /// Maximum delay.
    pub max_delay: Duration,
    /// Backoff strategy.
    pub strategy: BackoffStrategy,
    /// Jitter factor (0.0 to 1.0).
    pub jitter: f64,
    /// Retry on these errors.
    pub retry_on: Vec<RetryCondition>,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(30),
            strategy: BackoffStrategy::ExponentialBackoff { multiplier: 2.0 },
            jitter: 0.1,
            retry_on: vec![
                RetryCondition::Network,
                RetryCondition::RateLimit,
                RetryCondition::ServerError,
            ],
        }
    }
}

/// Backoff strategy.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BackoffStrategy {
    /// Fixed delay between retries.
    Fixed,
    /// Linear backoff (delay increases linearly).
    Linear { increment: f64 },
    /// Exponential backoff.
    ExponentialBackoff { multiplier: f64 },
    /// Decorrelated jitter (AWS style).
    DecorrelatedJitter { base: f64 },
    /// No delay.
    Immediate,
}

impl BackoffStrategy {
    /// Calculate delay for given attempt.
    pub fn delay(&self, attempt: u32, initial: Duration, max: Duration) -> Duration {
        let delay = match self {
            Self::Fixed => initial,
            Self::Linear { increment } => {
                Duration::from_secs_f64(initial.as_secs_f64() + (attempt as f64 * increment))
            }
            Self::ExponentialBackoff { multiplier } => {
                Duration::from_secs_f64(initial.as_secs_f64() * multiplier.powi(attempt as i32))
            }
            Self::DecorrelatedJitter { base } => {
                let mut rng = rand::thread_rng();
                let prev = initial.as_secs_f64() * base.powi(attempt.saturating_sub(1) as i32);
                Duration::from_secs_f64(rng.gen_range(initial.as_secs_f64()..=prev * 3.0))
            }
            Self::Immediate => Duration::ZERO,
        };

        delay.min(max)
    }
}

/// Condition for retrying.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryCondition {
    /// Network errors.
    Network,
    /// Rate limit errors.
    RateLimit,
    /// Server errors (5xx).
    ServerError,
    /// Timeout errors.
    Timeout,
    /// All errors.
    All,
}

impl RetryCondition {
    /// Check if error matches condition.
    pub fn matches(&self, error: &CortexError) -> bool {
        match self {
            Self::Network => matches!(
                error,
                CortexError::Network(_) | CortexError::ConnectionFailed { .. }
            ),
            Self::RateLimit => matches!(
                error,
                CortexError::RateLimitExceeded
                    | CortexError::RateLimit(_)
                    | CortexError::RateLimitWithRetryAfter { .. }
            ),
            Self::ServerError => matches!(error, CortexError::Provider(_)),
            Self::Timeout => matches!(error, CortexError::Timeout),
            Self::All => true,
        }
    }
}

/// Retry executor.
pub struct Retry {
    config: RetryConfig,
}

impl Retry {
    /// Create a new retry executor.
    pub fn new(config: RetryConfig) -> Self {
        Self { config }
    }

    /// Create with default config.
    pub fn default_retry() -> Self {
        Self::new(RetryConfig::default())
    }

    /// Execute with retry.
    /// Respects Retry-After headers from rate limit errors (HTTP 429).
    pub async fn execute<F, Fut, T>(&self, mut f: F) -> Result<T>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let mut attempt = 0;

        loop {
            attempt += 1;

            match f().await {
                Ok(result) => return Ok(result),
                Err(error) => {
                    // Check if we should retry
                    let should_retry = self.should_retry(&error, attempt);

                    if !should_retry {
                        return Err(error);
                    }

                    // Calculate delay, respecting Retry-After header if present
                    let delay = self.calculate_delay_with_retry_after(&error, attempt);

                    warn!(
                        attempt,
                        max_attempts = self.config.max_attempts,
                        delay_ms = delay.as_millis(),
                        error = %error,
                        "Retrying after error"
                    );

                    sleep(delay).await;
                }
            }
        }
    }

    /// Calculate delay, taking into account Retry-After header from rate limit errors.
    fn calculate_delay_with_retry_after(&self, error: &CortexError, attempt: u32) -> Duration {
        // Check if the error has a Retry-After value
        if let Some(retry_after_secs) = error.retry_after_secs() {
            let retry_after = Duration::from_secs(retry_after_secs);

            // Use the larger of Retry-After and our calculated backoff
            let backoff_delay = self.calculate_delay(attempt);

            // Apply Retry-After as minimum, then add some jitter on top
            let base_delay = retry_after.max(backoff_delay);

            // Log that we're respecting Retry-After
            debug!(
                retry_after_secs,
                backoff_ms = backoff_delay.as_millis(),
                final_delay_ms = base_delay.as_millis(),
                "Respecting Retry-After header from rate limit response"
            );

            base_delay
        } else {
            self.calculate_delay(attempt)
        }
    }

    /// Check if should retry.
    fn should_retry(&self, error: &CortexError, attempt: u32) -> bool {
        if attempt >= self.config.max_attempts {
            return false;
        }

        self.config.retry_on.iter().any(|c| c.matches(error))
    }

    /// Calculate delay for current attempt.
    fn calculate_delay(&self, attempt: u32) -> Duration {
        let base_delay =
            self.config
                .strategy
                .delay(attempt, self.config.initial_delay, self.config.max_delay);

        // Apply jitter
        if self.config.jitter > 0.0 {
            let mut rng = rand::thread_rng();
            let jitter_range = base_delay.as_secs_f64() * self.config.jitter;
            let jitter = rng.gen_range(-jitter_range..=jitter_range);
            Duration::from_secs_f64((base_delay.as_secs_f64() + jitter).max(0.0))
        } else {
            base_delay
        }
    }
}

/// Retry builder.
pub struct RetryBuilder {
    config: RetryConfig,
}

impl RetryBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            config: RetryConfig::default(),
        }
    }

    /// Set maximum attempts.
    pub fn max_attempts(mut self, n: u32) -> Self {
        self.config.max_attempts = n;
        self
    }

    /// Set initial delay.
    pub fn initial_delay(mut self, delay: Duration) -> Self {
        self.config.initial_delay = delay;
        self
    }

    /// Set maximum delay.
    pub fn max_delay(mut self, delay: Duration) -> Self {
        self.config.max_delay = delay;
        self
    }

    /// Set backoff strategy.
    pub fn strategy(mut self, strategy: BackoffStrategy) -> Self {
        self.config.strategy = strategy;
        self
    }

    /// Set jitter factor.
    pub fn jitter(mut self, factor: f64) -> Self {
        self.config.jitter = factor.clamp(0.0, 1.0);
        self
    }

    /// Retry on specific conditions.
    pub fn retry_on(mut self, conditions: Vec<RetryCondition>) -> Self {
        self.config.retry_on = conditions;
        self
    }

    /// Use exponential backoff.
    pub fn exponential(mut self) -> Self {
        self.config.strategy = BackoffStrategy::ExponentialBackoff { multiplier: 2.0 };
        self
    }

    /// Use linear backoff.
    pub fn linear(mut self, increment: f64) -> Self {
        self.config.strategy = BackoffStrategy::Linear { increment };
        self
    }

    /// Use fixed delay.
    pub fn fixed(mut self) -> Self {
        self.config.strategy = BackoffStrategy::Fixed;
        self
    }

    /// Build the retry executor.
    pub fn build(self) -> Retry {
        Retry::new(self.config)
    }
}

impl Default for RetryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience function for retrying with default config.
pub async fn retry<F, Fut, T>(f: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    Retry::default_retry().execute(f).await
}

/// Convenience function for retrying with custom attempts.
pub async fn retry_n<F, Fut, T>(n: u32, f: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    RetryBuilder::new().max_attempts(n).build().execute(f).await
}

/// Retry with exponential backoff.
pub async fn retry_exponential<F, Fut, T>(f: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    RetryBuilder::new().exponential().build().execute(f).await
}

/// Circuit breaker state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Circuit is closed (normal operation).
    Closed,
    /// Circuit is open (failing fast).
    Open,
    /// Circuit is half-open (testing recovery).
    HalfOpen,
}

/// Circuit breaker for preventing cascade failures.
pub struct CircuitBreaker {
    /// Failure threshold before opening.
    failure_threshold: u32,
    /// Success threshold for recovery.
    success_threshold: u32,
    /// Timeout for open state.
    timeout: Duration,
    /// Current state.
    state: tokio::sync::RwLock<CircuitState>,
    /// Failure count.
    failure_count: std::sync::atomic::AtomicU32,
    /// Success count (in half-open).
    success_count: std::sync::atomic::AtomicU32,
    /// Last failure time.
    last_failure: tokio::sync::RwLock<Option<std::time::Instant>>,
}

impl CircuitBreaker {
    /// Create a new circuit breaker.
    pub fn new(failure_threshold: u32, success_threshold: u32, timeout: Duration) -> Self {
        Self {
            failure_threshold,
            success_threshold,
            timeout,
            state: tokio::sync::RwLock::new(CircuitState::Closed),
            failure_count: std::sync::atomic::AtomicU32::new(0),
            success_count: std::sync::atomic::AtomicU32::new(0),
            last_failure: tokio::sync::RwLock::new(None),
        }
    }

    /// Create with default settings.
    pub fn default_breaker() -> Self {
        Self::new(5, 3, Duration::from_secs(30))
    }

    /// Get current state.
    pub async fn state(&self) -> CircuitState {
        self.maybe_transition().await;
        *self.state.read().await
    }

    /// Check if call is allowed.
    pub async fn is_allowed(&self) -> bool {
        self.maybe_transition().await;

        let state = *self.state.read().await;
        match state {
            CircuitState::Closed => true,
            CircuitState::Open => false,
            CircuitState::HalfOpen => true,
        }
    }

    /// Record a success.
    pub async fn record_success(&self) {
        let state = *self.state.read().await;

        match state {
            CircuitState::Closed => {
                self.failure_count
                    .store(0, std::sync::atomic::Ordering::Relaxed);
            }
            CircuitState::HalfOpen => {
                let count = self
                    .success_count
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    + 1;
                if count >= self.success_threshold {
                    *self.state.write().await = CircuitState::Closed;
                    self.failure_count
                        .store(0, std::sync::atomic::Ordering::Relaxed);
                    self.success_count
                        .store(0, std::sync::atomic::Ordering::Relaxed);
                    debug!("Circuit breaker closed after successful recovery");
                }
            }
            CircuitState::Open => {}
        }
    }

    /// Record a failure.
    pub async fn record_failure(&self) {
        let state = *self.state.read().await;

        match state {
            CircuitState::Closed => {
                let count = self
                    .failure_count
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    + 1;
                if count >= self.failure_threshold {
                    *self.state.write().await = CircuitState::Open;
                    *self.last_failure.write().await = Some(std::time::Instant::now());
                    warn!("Circuit breaker opened after {} failures", count);
                }
            }
            CircuitState::HalfOpen => {
                *self.state.write().await = CircuitState::Open;
                *self.last_failure.write().await = Some(std::time::Instant::now());
                self.success_count
                    .store(0, std::sync::atomic::Ordering::Relaxed);
                debug!("Circuit breaker re-opened after half-open failure");
            }
            CircuitState::Open => {}
        }
    }

    /// Execute with circuit breaker.
    pub async fn execute<F, Fut, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        if !self.is_allowed().await {
            return Err(CortexError::Provider("Circuit breaker is open".to_string()));
        }

        match f().await {
            Ok(result) => {
                self.record_success().await;
                Ok(result)
            }
            Err(error) => {
                self.record_failure().await;
                Err(error)
            }
        }
    }

    /// Transition state if needed.
    async fn maybe_transition(&self) {
        let state = *self.state.read().await;

        if state == CircuitState::Open
            && let Some(last) = *self.last_failure.read().await
            && last.elapsed() >= self.timeout
        {
            *self.state.write().await = CircuitState::HalfOpen;
            self.success_count
                .store(0, std::sync::atomic::Ordering::Relaxed);
            debug!("Circuit breaker entering half-open state");
        }
    }

    /// Force reset to closed state.
    pub async fn reset(&self) {
        *self.state.write().await = CircuitState::Closed;
        self.failure_count
            .store(0, std::sync::atomic::Ordering::Relaxed);
        self.success_count
            .store(0, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Retry with circuit breaker.
pub struct RetryWithBreaker {
    retry: Retry,
    breaker: CircuitBreaker,
}

impl RetryWithBreaker {
    /// Create new retry with circuit breaker.
    pub fn new(retry: Retry, breaker: CircuitBreaker) -> Self {
        Self { retry, breaker }
    }

    /// Execute with retry and circuit breaker.
    pub async fn execute<F, Fut, T>(&self, f: F) -> Result<T>
    where
        F: FnMut() -> Fut + Clone,
        Fut: Future<Output = Result<T>>,
    {
        if !self.breaker.is_allowed().await {
            return Err(CortexError::Provider("Circuit breaker is open".to_string()));
        }

        let func = f;
        let result = self.retry.execute(func).await;

        match &result {
            Ok(_) => self.breaker.record_success().await,
            Err(_) => self.breaker.record_failure().await,
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn test_backoff_strategies() {
        let initial = Duration::from_millis(100);
        let max = Duration::from_secs(10);

        // Fixed
        let fixed = BackoffStrategy::Fixed;
        assert_eq!(fixed.delay(1, initial, max), initial);
        assert_eq!(fixed.delay(5, initial, max), initial);

        // Exponential
        let exp = BackoffStrategy::ExponentialBackoff { multiplier: 2.0 };
        assert_eq!(exp.delay(0, initial, max), initial);
        assert_eq!(exp.delay(1, initial, max), Duration::from_millis(200));
        assert_eq!(exp.delay(2, initial, max), Duration::from_millis(400));
    }

    #[tokio::test]
    async fn test_retry_success() {
        let counter = AtomicU32::new(0);

        let result = retry(|| async {
            let count = counter.fetch_add(1, Ordering::Relaxed);
            if count < 2 {
                // RateLimitExceeded is retriable by default config
                Err(CortexError::RateLimitExceeded)
            } else {
                Ok(42)
            }
        })
        .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(counter.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        let counter = AtomicU32::new(0);

        let result: Result<()> = RetryBuilder::new()
            .max_attempts(2)
            .initial_delay(Duration::from_millis(1))
            .build()
            .execute(|| async {
                counter.fetch_add(1, Ordering::Relaxed);
                // RateLimitExceeded is retriable by default config
                Err(CortexError::RateLimitExceeded)
            })
            .await;

        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn test_circuit_breaker() {
        let breaker = CircuitBreaker::new(2, 1, Duration::from_millis(100));

        // Should start closed
        assert_eq!(breaker.state().await, CircuitState::Closed);

        // Record failures
        breaker.record_failure().await;
        assert_eq!(breaker.state().await, CircuitState::Closed);

        breaker.record_failure().await;
        assert_eq!(breaker.state().await, CircuitState::Open);

        // Should not be allowed
        assert!(!breaker.is_allowed().await);

        // Wait for timeout
        sleep(Duration::from_millis(150)).await;

        // Should be half-open
        assert_eq!(breaker.state().await, CircuitState::HalfOpen);

        // Record success
        breaker.record_success().await;
        assert_eq!(breaker.state().await, CircuitState::Closed);
    }

    #[test]
    fn test_builder() {
        let retry = RetryBuilder::new()
            .max_attempts(5)
            .exponential()
            .jitter(0.2)
            .build();

        assert_eq!(retry.config.max_attempts, 5);
        assert_eq!(retry.config.jitter, 0.2);
    }
}
