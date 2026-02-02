//! Health check utilities.
//!
//! Provides health checking, status monitoring, and
//! service health reporting.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::api_client::create_client_with_timeout;

/// Health status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum HealthStatus {
    /// Service is healthy.
    Healthy,
    /// Service is degraded but operational.
    Degraded,
    /// Service is unhealthy.
    Unhealthy,
    /// Status is unknown.
    #[default]
    Unknown,
}

impl HealthStatus {
    /// Check if healthy.
    pub fn is_healthy(&self) -> bool {
        *self == Self::Healthy
    }

    /// Check if operational.
    pub fn is_operational(&self) -> bool {
        matches!(self, Self::Healthy | Self::Degraded)
    }

    /// Get status name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Unhealthy => "unhealthy",
            Self::Unknown => "unknown",
        }
    }

    /// Get status symbol (text-based, no emoji).
    pub fn emoji(&self) -> &'static str {
        match self {
            Self::Healthy => "[OK]",
            Self::Degraded => "[WARN]",
            Self::Unhealthy => "[ERROR]",
            Self::Unknown => "[?]",
        }
    }
}

/// Health check result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckResult {
    /// Component name.
    pub component: String,
    /// Status.
    pub status: HealthStatus,
    /// Message.
    pub message: Option<String>,
    /// Duration.
    pub duration_ms: u64,
    /// Timestamp.
    pub timestamp: u64,
    /// Additional details.
    pub details: HashMap<String, serde_json::Value>,
}

impl HealthCheckResult {
    /// Create a healthy result.
    pub fn healthy(component: impl Into<String>, duration: Duration) -> Self {
        Self {
            component: component.into(),
            status: HealthStatus::Healthy,
            message: None,
            duration_ms: duration.as_millis() as u64,
            timestamp: timestamp_now(),
            details: HashMap::new(),
        }
    }

    /// Create a degraded result.
    pub fn degraded(
        component: impl Into<String>,
        message: impl Into<String>,
        duration: Duration,
    ) -> Self {
        Self {
            component: component.into(),
            status: HealthStatus::Degraded,
            message: Some(message.into()),
            duration_ms: duration.as_millis() as u64,
            timestamp: timestamp_now(),
            details: HashMap::new(),
        }
    }

    /// Create an unhealthy result.
    pub fn unhealthy(
        component: impl Into<String>,
        message: impl Into<String>,
        duration: Duration,
    ) -> Self {
        Self {
            component: component.into(),
            status: HealthStatus::Unhealthy,
            message: Some(message.into()),
            duration_ms: duration.as_millis() as u64,
            timestamp: timestamp_now(),
            details: HashMap::new(),
        }
    }

    /// Add detail.
    pub fn with_detail(mut self, key: impl Into<String>, value: impl Serialize) -> Self {
        if let Ok(v) = serde_json::to_value(value) {
            self.details.insert(key.into(), v);
        }
        self
    }
}

/// Health check trait.
#[async_trait::async_trait]
pub trait HealthCheck: Send + Sync {
    /// Get component name.
    fn name(&self) -> &str;

    /// Perform health check.
    async fn check(&self) -> HealthCheckResult;
}

/// System health report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    /// Overall status.
    pub status: HealthStatus,
    /// Individual check results.
    pub checks: Vec<HealthCheckResult>,
    /// Total duration.
    pub duration_ms: u64,
    /// Timestamp.
    pub timestamp: u64,
    /// Version info.
    pub version: Option<String>,
}

impl HealthReport {
    /// Create a new report.
    pub fn new(checks: Vec<HealthCheckResult>, duration: Duration) -> Self {
        let status = Self::aggregate_status(&checks);
        Self {
            status,
            checks,
            duration_ms: duration.as_millis() as u64,
            timestamp: timestamp_now(),
            version: None,
        }
    }

    /// Set version.
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// Aggregate status from checks.
    fn aggregate_status(checks: &[HealthCheckResult]) -> HealthStatus {
        if checks.is_empty() {
            return HealthStatus::Unknown;
        }

        let has_unhealthy = checks.iter().any(|c| c.status == HealthStatus::Unhealthy);
        let has_degraded = checks.iter().any(|c| c.status == HealthStatus::Degraded);
        let has_unknown = checks.iter().any(|c| c.status == HealthStatus::Unknown);

        if has_unhealthy {
            HealthStatus::Unhealthy
        } else if has_degraded {
            HealthStatus::Degraded
        } else if has_unknown {
            HealthStatus::Unknown
        } else {
            HealthStatus::Healthy
        }
    }

    /// Get healthy count.
    pub fn healthy_count(&self) -> usize {
        self.checks.iter().filter(|c| c.status.is_healthy()).count()
    }

    /// Get total count.
    pub fn total_count(&self) -> usize {
        self.checks.len()
    }

    /// Format as text.
    pub fn format(&self) -> String {
        let mut output = String::new();

        output.push_str(&format!(
            "Overall Status: {} {}\n",
            self.status.emoji(),
            self.status.name().to_uppercase()
        ));

        if let Some(ref version) = self.version {
            output.push_str(&format!("Version: {version}\n"));
        }

        output.push_str(&format!("Duration: {}ms\n\n", self.duration_ms));

        for check in &self.checks {
            output.push_str(&format!(
                "  {} {} ({}ms)",
                check.status.emoji(),
                check.component,
                check.duration_ms
            ));

            if let Some(ref msg) = check.message {
                output.push_str(&format!(": {msg}"));
            }
            output.push('\n');
        }

        output
    }
}

/// Health checker.
pub struct HealthChecker {
    /// Registered checks.
    checks: RwLock<Vec<Arc<dyn HealthCheck>>>,
    /// Deep checks (only run when deep=true).
    deep_checks: RwLock<Vec<Arc<dyn HealthCheck>>>,
    /// Timeout.
    timeout: Duration,
    /// Cache.
    cache: RwLock<Option<CachedReport>>,
    /// Cache TTL.
    cache_ttl: Duration,
}

impl HealthChecker {
    /// Create a new checker.
    pub fn new() -> Self {
        Self {
            checks: RwLock::new(Vec::new()),
            deep_checks: RwLock::new(Vec::new()),
            timeout: Duration::from_secs(30),
            cache: RwLock::new(None),
            cache_ttl: Duration::from_secs(5),
        }
    }

    /// Set timeout.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set cache TTL.
    pub fn cache_ttl(mut self, ttl: Duration) -> Self {
        self.cache_ttl = ttl;
        self
    }

    /// Register a check.
    pub async fn register(&self, check: Arc<dyn HealthCheck>) {
        self.checks.write().await.push(check);
    }

    /// Register a deep check (only run when deep=true).
    pub async fn register_deep(&self, check: Arc<dyn HealthCheck>) {
        self.deep_checks.write().await.push(check);
    }

    /// Run all checks.
    pub async fn check(&self) -> HealthReport {
        self.check_with_options(false).await
    }

    /// Run checks with options.
    /// When `deep` is true, also runs deep checks that test actual dependencies.
    pub async fn check_with_options(&self, deep: bool) -> HealthReport {
        // Check cache (only for non-deep checks)
        if !deep {
            if let Some(cached) = self.get_cached().await {
                return cached;
            }
        }

        let start = Instant::now();
        let checks = self.checks.read().await;
        let mut results = Vec::new();

        for check in checks.iter() {
            let result = check.check().await;
            results.push(result);
        }

        // Run deep checks if requested
        if deep {
            let deep_checks = self.deep_checks.read().await;
            for check in deep_checks.iter() {
                let result = check.check().await;
                results.push(result);
            }
        }

        let report = HealthReport::new(results, start.elapsed());

        // Cache result (only for non-deep checks)
        if !deep {
            self.cache_report(&report).await;
        }

        report
    }

    /// Get cached report.
    async fn get_cached(&self) -> Option<HealthReport> {
        let cache = self.cache.read().await;
        if let Some(ref cached) = *cache
            && cached.timestamp.elapsed() < self.cache_ttl
        {
            return Some(cached.report.clone());
        }
        None
    }

    /// Cache report.
    async fn cache_report(&self, report: &HealthReport) {
        *self.cache.write().await = Some(CachedReport {
            report: report.clone(),
            timestamp: Instant::now(),
        });
    }

    /// Clear cache.
    pub async fn clear_cache(&self) {
        *self.cache.write().await = None;
    }
}

impl Default for HealthChecker {
    fn default() -> Self {
        Self::new()
    }
}

/// Cached report.
struct CachedReport {
    report: HealthReport,
    timestamp: Instant,
}

/// Simple health check.
pub struct SimpleHealthCheck {
    name: String,
    check_fn: Box<dyn Fn() -> bool + Send + Sync>,
}

impl SimpleHealthCheck {
    /// Create a new check.
    pub fn new<F>(name: impl Into<String>, check_fn: F) -> Self
    where
        F: Fn() -> bool + Send + Sync + 'static,
    {
        Self {
            name: name.into(),
            check_fn: Box::new(check_fn),
        }
    }
}

#[async_trait::async_trait]
impl HealthCheck for SimpleHealthCheck {
    fn name(&self) -> &str {
        &self.name
    }

    async fn check(&self) -> HealthCheckResult {
        let start = Instant::now();
        let healthy = (self.check_fn)();
        let duration = start.elapsed();

        if healthy {
            HealthCheckResult::healthy(&self.name, duration)
        } else {
            HealthCheckResult::unhealthy(&self.name, "Check failed", duration)
        }
    }
}

/// HTTP health check.
pub struct HttpHealthCheck {
    name: String,
    url: String,
    timeout: Duration,
}

impl HttpHealthCheck {
    /// Create a new check.
    pub fn new(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
            timeout: Duration::from_secs(5),
        }
    }

    /// Set timeout.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

#[async_trait::async_trait]
impl HealthCheck for HttpHealthCheck {
    fn name(&self) -> &str {
        &self.name
    }

    async fn check(&self) -> HealthCheckResult {
        let start = Instant::now();

        let client = create_client_with_timeout(self.timeout);

        let client = match client {
            Ok(c) => c,
            Err(e) => {
                return HealthCheckResult::unhealthy(
                    &self.name,
                    format!("Client error: {e}"),
                    start.elapsed(),
                );
            }
        };

        match client.get(&self.url).send().await {
            Ok(response) => {
                let duration = start.elapsed();
                let status = response.status();

                if status.is_success() {
                    HealthCheckResult::healthy(&self.name, duration)
                        .with_detail("status_code", status.as_u16())
                } else {
                    HealthCheckResult::unhealthy(&self.name, format!("HTTP {status}"), duration)
                        .with_detail("status_code", status.as_u16())
                }
            }
            Err(e) => HealthCheckResult::unhealthy(
                &self.name,
                format!("Request failed: {e}"),
                start.elapsed(),
            ),
        }
    }
}

/// Disk space check.
pub struct DiskSpaceCheck {
    name: String,
    path: std::path::PathBuf,
    min_free_bytes: u64,
    warn_free_bytes: u64,
}

impl DiskSpaceCheck {
    /// Create a new check.
    pub fn new(name: impl Into<String>, path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
            min_free_bytes: 1024 * 1024 * 100,   // 100MB
            warn_free_bytes: 1024 * 1024 * 1024, // 1GB
        }
    }

    /// Set minimum free bytes.
    pub fn min_free(mut self, bytes: u64) -> Self {
        self.min_free_bytes = bytes;
        self
    }

    /// Set warning threshold.
    pub fn warn_free(mut self, bytes: u64) -> Self {
        self.warn_free_bytes = bytes;
        self
    }
}

#[async_trait::async_trait]
impl HealthCheck for DiskSpaceCheck {
    fn name(&self) -> &str {
        &self.name
    }

    async fn check(&self) -> HealthCheckResult {
        let start = Instant::now();

        // Get available space (platform-specific, simplified here)
        let available = get_available_space(&self.path).unwrap_or(0);
        let duration = start.elapsed();

        if available < self.min_free_bytes {
            HealthCheckResult::unhealthy(
                &self.name,
                format!("Low disk space: {available} bytes available"),
                duration,
            )
            .with_detail("available_bytes", available)
        } else if available < self.warn_free_bytes {
            HealthCheckResult::degraded(
                &self.name,
                format!("Disk space warning: {available} bytes available"),
                duration,
            )
            .with_detail("available_bytes", available)
        } else {
            HealthCheckResult::healthy(&self.name, duration)
                .with_detail("available_bytes", available)
        }
    }
}

/// Get available disk space (simplified).
fn get_available_space(path: &std::path::Path) -> Option<u64> {
    // This is a simplified implementation
    // In production, use platform-specific APIs
    if path.exists() {
        Some(10 * 1024 * 1024 * 1024) // Placeholder: 10GB
    } else {
        None
    }
}

/// Get current timestamp.
fn timestamp_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// API connectivity health check.
/// Verifies that the API endpoint is reachable and responding.
pub struct ApiConnectivityCheck {
    name: String,
    api_url: String,
    timeout: Duration,
}

impl ApiConnectivityCheck {
    /// Create a new API connectivity check.
    pub fn new(name: impl Into<String>, api_url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            api_url: api_url.into(),
            timeout: Duration::from_secs(10),
        }
    }

    /// Set timeout.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

#[async_trait::async_trait]
impl HealthCheck for ApiConnectivityCheck {
    fn name(&self) -> &str {
        &self.name
    }

    async fn check(&self) -> HealthCheckResult {
        let start = Instant::now();

        let client = match create_client_with_timeout(self.timeout) {
            Ok(c) => c,
            Err(e) => {
                return HealthCheckResult::unhealthy(
                    &self.name,
                    format!("Failed to create client: {e}"),
                    start.elapsed(),
                );
            }
        };

        // Try to reach the API endpoint
        match client.get(&self.api_url).send().await {
            Ok(response) => {
                let duration = start.elapsed();
                let status = response.status();

                // Any response (including 401 Unauthorized) means the API is reachable
                if status.is_success() || status.is_client_error() {
                    HealthCheckResult::healthy(&self.name, duration)
                        .with_detail("status_code", status.as_u16())
                        .with_detail("api_url", self.api_url.clone())
                } else if status.is_server_error() {
                    HealthCheckResult::degraded(
                        &self.name,
                        format!("API returned server error: HTTP {status}"),
                        duration,
                    )
                    .with_detail("status_code", status.as_u16())
                } else {
                    HealthCheckResult::healthy(&self.name, duration)
                        .with_detail("status_code", status.as_u16())
                }
            }
            Err(e) => {
                let duration = start.elapsed();
                let message = if e.is_timeout() {
                    "API request timed out".to_string()
                } else if e.is_connect() {
                    "Failed to connect to API".to_string()
                } else {
                    format!("API connectivity error: {e}")
                };
                HealthCheckResult::unhealthy(&self.name, message, duration)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_status() {
        assert!(HealthStatus::Healthy.is_healthy());
        assert!(HealthStatus::Healthy.is_operational());
        assert!(HealthStatus::Degraded.is_operational());
        assert!(!HealthStatus::Unhealthy.is_operational());
    }

    #[test]
    fn test_health_check_result() {
        let result = HealthCheckResult::healthy("test", Duration::from_millis(50))
            .with_detail("key", "value");

        assert_eq!(result.status, HealthStatus::Healthy);
        assert_eq!(result.duration_ms, 50);
        assert!(result.details.contains_key("key"));
    }

    #[test]
    fn test_health_report() {
        let checks = vec![
            HealthCheckResult::healthy("check1", Duration::from_millis(10)),
            HealthCheckResult::healthy("check2", Duration::from_millis(20)),
        ];

        let report = HealthReport::new(checks, Duration::from_millis(30));
        assert_eq!(report.status, HealthStatus::Healthy);
        assert_eq!(report.healthy_count(), 2);
    }

    #[test]
    fn test_health_report_degraded() {
        let checks = vec![
            HealthCheckResult::healthy("check1", Duration::from_millis(10)),
            HealthCheckResult::degraded("check2", "warning", Duration::from_millis(20)),
        ];

        let report = HealthReport::new(checks, Duration::from_millis(30));
        assert_eq!(report.status, HealthStatus::Degraded);
    }

    #[test]
    fn test_health_report_unhealthy() {
        let checks = vec![
            HealthCheckResult::healthy("check1", Duration::from_millis(10)),
            HealthCheckResult::unhealthy("check2", "error", Duration::from_millis(20)),
        ];

        let report = HealthReport::new(checks, Duration::from_millis(30));
        assert_eq!(report.status, HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn test_simple_health_check() {
        let check = SimpleHealthCheck::new("test", || true);
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Healthy);

        let check = SimpleHealthCheck::new("test", || false);
        let result = check.check().await;
        assert_eq!(result.status, HealthStatus::Unhealthy);
    }

    #[tokio::test]
    async fn test_health_checker() {
        let checker = HealthChecker::new();

        let check = Arc::new(SimpleHealthCheck::new("test", || true));
        checker.register(check).await;

        let report = checker.check().await;
        assert_eq!(report.status, HealthStatus::Healthy);
        assert_eq!(report.checks.len(), 1);
    }
}
