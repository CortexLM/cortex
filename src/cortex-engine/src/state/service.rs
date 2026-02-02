//! Service state management.
//!
//! Tracks the state of external services, providers, and dependencies.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::error::{CortexError, Result};

/// Service state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceState {
    /// Service name.
    pub name: String,
    /// Service type.
    pub service_type: ServiceType,
    /// Current status.
    pub status: ServiceStatus,
    /// Service endpoint.
    pub endpoint: Option<String>,
    /// Last health check.
    pub last_health_check: Option<u64>,
    /// Last successful request.
    pub last_success: Option<u64>,
    /// Last error.
    pub last_error: Option<ServiceError>,
    /// Request count.
    pub request_count: u64,
    /// Error count.
    pub error_count: u64,
    /// Average latency in milliseconds.
    pub avg_latency_ms: u64,
    /// Service metadata.
    pub metadata: HashMap<String, String>,
}

impl ServiceState {
    /// Create a new service state.
    pub fn new(name: impl Into<String>, service_type: ServiceType) -> Self {
        Self {
            name: name.into(),
            service_type,
            status: ServiceStatus::Unknown,
            endpoint: None,
            last_health_check: None,
            last_success: None,
            last_error: None,
            request_count: 0,
            error_count: 0,
            avg_latency_ms: 0,
            metadata: HashMap::new(),
        }
    }

    /// Set endpoint.
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    /// Set as available.
    pub fn set_available(&mut self) {
        self.status = ServiceStatus::Available;
        self.last_health_check = Some(timestamp_now());
    }

    /// Set as unavailable.
    pub fn set_unavailable(&mut self, reason: impl Into<String>) {
        self.status = ServiceStatus::Unavailable;
        self.last_error = Some(ServiceError {
            message: reason.into(),
            timestamp: timestamp_now(),
            code: None,
        });
    }

    /// Set as degraded.
    pub fn set_degraded(&mut self, reason: impl Into<String>) {
        self.status = ServiceStatus::Degraded;
        self.last_error = Some(ServiceError {
            message: reason.into(),
            timestamp: timestamp_now(),
            code: None,
        });
    }

    /// Record a successful request.
    pub fn record_success(&mut self, latency_ms: u64) {
        self.request_count += 1;
        self.last_success = Some(timestamp_now());

        // Update average latency
        let total = self.avg_latency_ms * (self.request_count - 1) + latency_ms;
        self.avg_latency_ms = total / self.request_count;

        // Clear error status if it was temporary
        if self.status == ServiceStatus::Unavailable {
            self.status = ServiceStatus::Available;
        }
    }

    /// Record a failed request.
    pub fn record_error(&mut self, error: impl Into<String>, code: Option<String>) {
        self.request_count += 1;
        self.error_count += 1;
        self.last_error = Some(ServiceError {
            message: error.into(),
            timestamp: timestamp_now(),
            code,
        });

        // Calculate error rate
        let error_rate = self.error_count as f64 / self.request_count as f64;
        if error_rate > 0.5 {
            self.status = ServiceStatus::Unavailable;
        } else if error_rate > 0.1 {
            self.status = ServiceStatus::Degraded;
        }
    }

    /// Check if service is healthy.
    pub fn is_healthy(&self) -> bool {
        matches!(self.status, ServiceStatus::Available)
    }

    /// Get error rate.
    pub fn error_rate(&self) -> f64 {
        if self.request_count > 0 {
            self.error_count as f64 / self.request_count as f64
        } else {
            0.0
        }
    }

    /// Get uptime since last error.
    pub fn uptime_since_error(&self) -> Option<Duration> {
        let now = timestamp_now();
        self.last_error
            .as_ref()
            .map(|e| Duration::from_secs(now.saturating_sub(e.timestamp)))
    }
}

/// Service type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceType {
    /// LLM provider.
    Provider,
    /// MCP server.
    McpServer,
    /// Database.
    Database,
    /// Cache.
    Cache,
    /// External API.
    ExternalApi,
    /// File system.
    FileSystem,
    /// Git service.
    Git,
    /// Authentication service.
    Auth,
    /// Custom service.
    Custom,
}

/// Service status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ServiceStatus {
    /// Status unknown.
    #[default]
    Unknown,
    /// Service starting.
    Starting,
    /// Service available.
    Available,
    /// Service degraded but functional.
    Degraded,
    /// Service unavailable.
    Unavailable,
    /// Service stopping.
    Stopping,
    /// Service stopped.
    Stopped,
}

/// Service error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceError {
    /// Error message.
    pub message: String,
    /// Timestamp.
    pub timestamp: u64,
    /// Error code.
    pub code: Option<String>,
}

/// Service manager.
pub struct ServiceManager {
    /// Services indexed by name.
    services: RwLock<HashMap<String, ServiceState>>,
    /// Health check tasks.
    health_checks: RwLock<HashMap<String, HealthCheckConfig>>,
}

impl ServiceManager {
    /// Create a new service manager.
    pub fn new() -> Self {
        Self {
            services: RwLock::new(HashMap::new()),
            health_checks: RwLock::new(HashMap::new()),
        }
    }

    /// Register a service.
    pub async fn register(&self, state: ServiceState) {
        let name = state.name.clone();
        self.services.write().await.insert(name, state);
    }

    /// Get a service.
    pub async fn get(&self, name: &str) -> Option<ServiceState> {
        self.services.read().await.get(name).cloned()
    }

    /// Get service status.
    pub async fn status(&self, name: &str) -> Option<ServiceStatus> {
        self.services.read().await.get(name).map(|s| s.status)
    }

    /// Update a service.
    pub async fn update(&self, name: &str, f: impl FnOnce(&mut ServiceState)) -> Result<()> {
        let mut services = self.services.write().await;
        let service = services
            .get_mut(name)
            .ok_or_else(|| CortexError::NotFound(format!("Service not found: {name}")))?;
        f(service);
        Ok(())
    }

    /// List all services.
    pub async fn list(&self) -> Vec<ServiceInfo> {
        self.services
            .read()
            .await
            .values()
            .map(|s| ServiceInfo {
                name: s.name.clone(),
                service_type: s.service_type,
                status: s.status,
                error_rate: s.error_rate(),
            })
            .collect()
    }

    /// Get healthy services.
    pub async fn healthy(&self) -> Vec<String> {
        self.services
            .read()
            .await
            .iter()
            .filter(|(_, s)| s.is_healthy())
            .map(|(n, _)| n.clone())
            .collect()
    }

    /// Get unhealthy services.
    pub async fn unhealthy(&self) -> Vec<String> {
        self.services
            .read()
            .await
            .iter()
            .filter(|(_, s)| !s.is_healthy())
            .map(|(n, _)| n.clone())
            .collect()
    }

    /// Configure health check for a service.
    pub async fn configure_health_check(&self, name: &str, config: HealthCheckConfig) {
        self.health_checks
            .write()
            .await
            .insert(name.to_string(), config);
    }

    /// Run health check for a service.
    pub async fn check_health(&self, name: &str) -> Result<bool> {
        let health_checks = self.health_checks.read().await;
        let config = health_checks.get(name);

        if let Some(config) = config {
            let healthy = (config.check_fn)().await;
            drop(health_checks);

            self.update(name, |s| {
                s.last_health_check = Some(timestamp_now());
                if healthy {
                    s.set_available();
                } else {
                    s.set_unavailable("Health check failed");
                }
            })
            .await?;

            Ok(healthy)
        } else {
            Err(CortexError::NotFound(format!(
                "No health check configured for: {name}"
            )))
        }
    }

    /// Run all health checks.
    pub async fn check_all_health(&self) -> HashMap<String, bool> {
        let names: Vec<_> = self.health_checks.read().await.keys().cloned().collect();
        let mut results = HashMap::new();

        for name in names {
            if let Ok(healthy) = self.check_health(&name).await {
                results.insert(name, healthy);
            }
        }

        results
    }

    /// Get service metrics.
    pub async fn metrics(&self) -> ServiceMetrics {
        let services = self.services.read().await;

        let total = services.len();
        let healthy = services.values().filter(|s| s.is_healthy()).count();
        let total_requests: u64 = services.values().map(|s| s.request_count).sum();
        let total_errors: u64 = services.values().map(|s| s.error_count).sum();
        let avg_latency = if !services.is_empty() {
            services.values().map(|s| s.avg_latency_ms).sum::<u64>() / services.len() as u64
        } else {
            0
        };

        ServiceMetrics {
            total_services: total,
            healthy_services: healthy,
            unhealthy_services: total - healthy,
            total_requests,
            total_errors,
            error_rate: if total_requests > 0 {
                total_errors as f64 / total_requests as f64
            } else {
                0.0
            },
            avg_latency_ms: avg_latency,
        }
    }
}

impl Default for ServiceManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Service info for listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceInfo {
    /// Service name.
    pub name: String,
    /// Service type.
    pub service_type: ServiceType,
    /// Current status.
    pub status: ServiceStatus,
    /// Error rate.
    pub error_rate: f64,
}

/// Health check configuration.
pub struct HealthCheckConfig {
    /// Check interval.
    pub interval: Duration,
    /// Check timeout.
    pub timeout: Duration,
    /// Check function.
    pub check_fn: Box<
        dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>> + Send + Sync,
    >,
}

impl HealthCheckConfig {
    /// Create a new health check config.
    pub fn new<F, Fut>(interval: Duration, timeout: Duration, check_fn: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = bool> + Send + 'static,
    {
        Self {
            interval,
            timeout,
            check_fn: Box::new(move || Box::pin(check_fn())),
        }
    }
}

/// Service metrics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServiceMetrics {
    /// Total services.
    pub total_services: usize,
    /// Healthy services.
    pub healthy_services: usize,
    /// Unhealthy services.
    pub unhealthy_services: usize,
    /// Total requests.
    pub total_requests: u64,
    /// Total errors.
    pub total_errors: u64,
    /// Overall error rate.
    pub error_rate: f64,
    /// Average latency.
    pub avg_latency_ms: u64,
}

/// Get current timestamp.
fn timestamp_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_state() {
        let mut state = ServiceState::new("openai", ServiceType::Provider);
        assert_eq!(state.status, ServiceStatus::Unknown);

        state.set_available();
        assert!(state.is_healthy());

        state.record_success(100);
        assert_eq!(state.request_count, 1);
        assert_eq!(state.avg_latency_ms, 100);

        state.record_error("API error", Some("500".to_string()));
        assert_eq!(state.error_count, 1);
    }

    #[test]
    fn test_error_rate() {
        let mut state = ServiceState::new("test", ServiceType::Custom);

        // No requests
        assert_eq!(state.error_rate(), 0.0);

        // 2 successes, 1 error = 33% error rate
        state.record_success(100);
        state.record_success(100);
        state.record_error("error", None);

        let rate = state.error_rate();
        assert!((rate - 0.333).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_service_manager() {
        let manager = ServiceManager::new();

        let state = ServiceState::new("test", ServiceType::Custom);
        manager.register(state).await;

        assert!(manager.get("test").await.is_some());

        manager
            .update("test", |s| {
                s.set_available();
            })
            .await
            .unwrap();

        let healthy = manager.healthy().await;
        assert!(healthy.contains(&"test".to_string()));
    }

    #[tokio::test]
    async fn test_service_metrics() {
        let manager = ServiceManager::new();

        let mut state1 = ServiceState::new("s1", ServiceType::Custom);
        state1.set_available();
        state1.record_success(100);

        let mut state2 = ServiceState::new("s2", ServiceType::Custom);
        state2.set_unavailable("error");

        manager.register(state1).await;
        manager.register(state2).await;

        let metrics = manager.metrics().await;
        assert_eq!(metrics.total_services, 2);
        assert_eq!(metrics.healthy_services, 1);
        assert_eq!(metrics.unhealthy_services, 1);
    }
}
