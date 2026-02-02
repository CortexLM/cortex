//! Rate limit tracking.

use crate::{RateLimitInfo, UsageStats};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Tracker for rate limits across providers.
pub struct RateLimitTracker {
    /// Rate limits by provider.
    limits: Arc<RwLock<HashMap<String, RateLimitInfo>>>,
    /// Usage stats by provider.
    usage: Arc<RwLock<HashMap<String, UsageStats>>>,
    /// Current provider.
    current_provider: Arc<RwLock<Option<String>>>,
}

impl RateLimitTracker {
    pub fn new() -> Self {
        Self {
            limits: Arc::new(RwLock::new(HashMap::new())),
            usage: Arc::new(RwLock::new(HashMap::new())),
            current_provider: Arc::new(RwLock::new(None)),
        }
    }

    /// Set the current provider.
    pub async fn set_provider(&self, provider: &str) {
        *self.current_provider.write().await = Some(provider.to_string());

        // Initialize usage if not exists
        let mut usage = self.usage.write().await;
        usage
            .entry(provider.to_string())
            .or_insert_with(UsageStats::new);
    }

    /// Update rate limits from response headers.
    pub async fn update_limits(&self, provider: &str, info: RateLimitInfo) {
        self.limits.write().await.insert(provider.to_string(), info);
    }

    /// Record usage from a request.
    pub async fn record_usage(
        &self,
        provider: &str,
        input_tokens: u64,
        output_tokens: u64,
        cached_tokens: u64,
    ) {
        let mut usage = self.usage.write().await;
        let stats = usage
            .entry(provider.to_string())
            .or_insert_with(UsageStats::new);
        stats.add_request(input_tokens, output_tokens, cached_tokens);
    }

    /// Get current rate limits.
    pub async fn get_limits(&self, provider: &str) -> Option<RateLimitInfo> {
        self.limits.read().await.get(provider).cloned()
    }

    /// Get current usage stats.
    pub async fn get_usage(&self, provider: &str) -> Option<UsageStats> {
        self.usage.read().await.get(provider).cloned()
    }

    /// Get limits for current provider.
    pub async fn get_current_limits(&self) -> Option<RateLimitInfo> {
        let provider = self.current_provider.read().await.clone()?;
        self.get_limits(&provider).await
    }

    /// Get usage for current provider.
    pub async fn get_current_usage(&self) -> Option<UsageStats> {
        let provider = self.current_provider.read().await.clone()?;
        self.get_usage(&provider).await
    }

    /// Check if currently rate limited.
    pub async fn is_rate_limited(&self) -> bool {
        if let Some(limits) = self.get_current_limits().await {
            limits.is_rate_limited()
        } else {
            false
        }
    }

    /// Check if approaching rate limit.
    pub async fn is_approaching_limit(&self) -> bool {
        if let Some(limits) = self.get_current_limits().await {
            limits.is_approaching_limit()
        } else {
            false
        }
    }

    /// Get all providers with their limits.
    pub async fn all_limits(&self) -> HashMap<String, RateLimitInfo> {
        self.limits.read().await.clone()
    }

    /// Get all providers with their usage.
    pub async fn all_usage(&self) -> HashMap<String, UsageStats> {
        self.usage.read().await.clone()
    }

    /// Reset usage for a provider.
    pub async fn reset_usage(&self, provider: &str) {
        self.usage
            .write()
            .await
            .insert(provider.to_string(), UsageStats::new());
    }
}

impl Default for RateLimitTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_tracker() {
        let tracker = RateLimitTracker::new();

        tracker.set_provider("openai").await;
        tracker.record_usage("openai", 100, 50, 20).await;

        let usage = tracker.get_current_usage().await.unwrap();
        assert_eq!(usage.total_requests, 1);
        assert_eq!(usage.total_input_tokens, 100);
        assert_eq!(usage.total_output_tokens, 50);
    }
}
