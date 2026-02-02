//! Network proxy state and metrics.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Metrics for network proxy requests.
#[derive(Debug, Default)]
pub struct RequestMetrics {
    /// Total number of requests.
    pub total_requests: AtomicU64,

    /// Number of allowed requests.
    pub allowed_requests: AtomicU64,

    /// Number of blocked requests.
    pub blocked_requests: AtomicU64,

    /// Total bytes transferred (approximate).
    pub bytes_transferred: AtomicU64,
}

impl RequestMetrics {
    /// Create new metrics.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an allowed request.
    pub fn record_allowed(&self, bytes: u64) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.allowed_requests.fetch_add(1, Ordering::Relaxed);
        self.bytes_transferred.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Record a blocked request.
    pub fn record_blocked(&self) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.blocked_requests.fetch_add(1, Ordering::Relaxed);
    }

    /// Get total requests.
    pub fn total(&self) -> u64 {
        self.total_requests.load(Ordering::Relaxed)
    }

    /// Get allowed requests count.
    pub fn allowed(&self) -> u64 {
        self.allowed_requests.load(Ordering::Relaxed)
    }

    /// Get blocked requests count.
    pub fn blocked(&self) -> u64 {
        self.blocked_requests.load(Ordering::Relaxed)
    }

    /// Get bytes transferred.
    pub fn bytes(&self) -> u64 {
        self.bytes_transferred.load(Ordering::Relaxed)
    }

    /// Get a snapshot of the metrics.
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            total_requests: self.total(),
            allowed_requests: self.allowed(),
            blocked_requests: self.blocked(),
            bytes_transferred: self.bytes(),
        }
    }
}

/// Snapshot of metrics at a point in time.
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub total_requests: u64,
    pub allowed_requests: u64,
    pub blocked_requests: u64,
    pub bytes_transferred: u64,
}

/// State for the network proxy.
pub struct NetworkProxyState {
    /// Policy engine.
    policy: super::PolicyEngine,

    /// Request metrics.
    metrics: Arc<RequestMetrics>,

    /// Whether the proxy is active.
    active: std::sync::atomic::AtomicBool,
}

impl NetworkProxyState {
    /// Create new proxy state.
    pub fn new(policy: super::PolicyEngine) -> Self {
        Self {
            policy,
            metrics: Arc::new(RequestMetrics::new()),
            active: std::sync::atomic::AtomicBool::new(true),
        }
    }

    /// Get the policy engine.
    pub fn policy(&self) -> &super::PolicyEngine {
        &self.policy
    }

    /// Get the metrics.
    pub fn metrics(&self) -> &Arc<RequestMetrics> {
        &self.metrics
    }

    /// Check if the proxy is active.
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// Set the proxy active state.
    pub fn set_active(&self, active: bool) {
        self.active.store(active, Ordering::Release);
    }

    /// Check if a host is blocked and record metrics.
    pub async fn check_and_record(&self, method: &str, host: &str, port: u16) -> super::Result<()> {
        if !self.is_active() {
            self.metrics.record_blocked();
            return Err(super::NetworkProxyError::Internal(
                "Proxy is not active".to_string(),
            ));
        }

        match self.policy.validate_request(method, host, port).await {
            Ok(()) => {
                // Will record bytes after transfer
                Ok(())
            }
            Err(e) => {
                self.metrics.record_blocked();
                Err(e)
            }
        }
    }

    /// Record successful transfer.
    pub fn record_success(&self, bytes: u64) {
        self.metrics.record_allowed(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics() {
        let metrics = RequestMetrics::new();

        metrics.record_allowed(1000);
        metrics.record_allowed(500);
        metrics.record_blocked();

        assert_eq!(metrics.total(), 3);
        assert_eq!(metrics.allowed(), 2);
        assert_eq!(metrics.blocked(), 1);
        assert_eq!(metrics.bytes(), 1500);
    }
}
