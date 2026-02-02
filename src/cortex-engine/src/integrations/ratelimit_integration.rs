//! Rate limit integration for cortex-core.
//!
//! Connects cortex-ratelimits to track and display API rate limits.

use cortex_ratelimits::display::format_rate_limits_compact;
use cortex_ratelimits::{
    RateLimitDisplay, RateLimitInfo, RateLimitTracker, UsageStats, format_rate_limits,
};
use std::collections::HashMap;
use std::sync::Arc;

/// Rate limit integration.
pub struct RatelimitIntegration {
    tracker: Arc<RateLimitTracker>,
}

impl RatelimitIntegration {
    /// Create a new rate limit integration.
    pub fn new() -> Self {
        Self {
            tracker: Arc::new(RateLimitTracker::new()),
        }
    }

    /// Set the current provider.
    pub async fn set_provider(&self, provider: &str) {
        self.tracker.set_provider(provider).await;
    }

    /// Update rate limits from response headers.
    pub async fn update_from_headers(&self, provider: &str, headers: &HashMap<String, String>) {
        let info = cortex_ratelimits::limits::parse_rate_limit_headers(headers);
        self.tracker.update_limits(provider, info).await;
    }

    /// Update rate limits directly.
    pub async fn update_limits(&self, provider: &str, info: RateLimitInfo) {
        self.tracker.update_limits(provider, info).await;
    }

    /// Record usage from a request.
    pub async fn record_usage(
        &self,
        provider: &str,
        input_tokens: u64,
        output_tokens: u64,
        cached_tokens: u64,
    ) {
        self.tracker
            .record_usage(provider, input_tokens, output_tokens, cached_tokens)
            .await;
    }

    /// Get current rate limits.
    pub async fn get_limits(&self) -> Option<RateLimitInfo> {
        self.tracker.get_current_limits().await
    }

    /// Get current usage stats.
    pub async fn get_usage(&self) -> Option<UsageStats> {
        self.tracker.get_current_usage().await
    }

    /// Check if rate limited.
    pub async fn is_rate_limited(&self) -> bool {
        self.tracker.is_rate_limited().await
    }

    /// Check if approaching limit.
    pub async fn is_approaching_limit(&self) -> bool {
        self.tracker.is_approaching_limit().await
    }

    /// Format rate limits for full display.
    pub async fn format_full(&self) -> Vec<String> {
        let limits = self.tracker.get_current_limits().await.unwrap_or_default();
        let usage = self.tracker.get_current_usage().await.unwrap_or_default();
        format_rate_limits(&limits, &usage, &RateLimitDisplay::default())
    }

    /// Format rate limits for status bar.
    pub async fn format_compact(&self) -> String {
        let limits = self.tracker.get_current_limits().await.unwrap_or_default();
        format_rate_limits_compact(&limits)
    }

    /// Get the underlying tracker.
    pub fn tracker(&self) -> Arc<RateLimitTracker> {
        Arc::clone(&self.tracker)
    }
}

impl Default for RatelimitIntegration {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for RatelimitIntegration {
    fn clone(&self) -> Self {
        Self {
            tracker: Arc::clone(&self.tracker),
        }
    }
}
