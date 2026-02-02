//! mDNS/Bonjour service discovery for Cortex server.
//!
//! This module provides:
//! - `MdnsPublisher`: Publishes the Cortex server as a discoverable service on the local network
//! - `MdnsDiscovery`: Discovers other Cortex servers on the local network
//!
//! The service is published under the `_cortex._tcp.local.` service type.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::error::{AppError, AppResult};

/// The mDNS service type for Cortex servers.
pub const SERVICE_TYPE: &str = "_cortex._tcp.local.";

/// mDNS publisher for advertising the Cortex server on the local network.
pub struct MdnsPublisher {
    daemon: ServiceDaemon,
    service_fullname: String,
    registered: Arc<RwLock<bool>>,
}

impl MdnsPublisher {
    /// Creates a new mDNS publisher for the given port.
    ///
    /// # Arguments
    /// * `port` - The port the Cortex server is listening on
    /// * `service_name` - Optional custom service name (defaults to "cortex-{port}")
    pub fn new(port: u16, service_name: Option<String>) -> AppResult<Self> {
        let daemon = ServiceDaemon::new()
            .map_err(|e| AppError::Internal(format!("Failed to create mDNS daemon: {}", e)))?;

        let hostname = gethostname::gethostname().to_string_lossy().to_string();

        let instance_name = service_name.unwrap_or_else(|| format!("cortex-{}", port));
        let service_fullname = format!("{}.{}", instance_name, SERVICE_TYPE);

        info!(
            "mDNS publisher created for {} on host {}",
            service_fullname, hostname
        );

        Ok(Self {
            daemon,
            service_fullname,
            registered: Arc::new(RwLock::new(false)),
        })
    }

    /// Publishes the service on the network.
    ///
    /// This makes the Cortex server discoverable by other clients on the local network.
    pub async fn publish(&self, port: u16) -> AppResult<()> {
        let hostname = gethostname::gethostname().to_string_lossy().to_string();

        let host_fullname = format!("{}.local.", hostname);

        // Get local IP addresses
        let addresses = get_local_addresses()?;

        if addresses.is_empty() {
            return Err(AppError::Internal(
                "No local IP addresses found for mDNS publishing".to_string(),
            ));
        }

        // Build service properties
        let mut properties = HashMap::new();
        properties.insert("path".to_string(), "/".to_string());
        properties.insert("version".to_string(), env!("CARGO_PKG_VERSION").to_string());
        properties.insert("api".to_string(), "v1".to_string());

        // Create service info with all local addresses
        let service = ServiceInfo::new(
            SERVICE_TYPE,
            self.service_fullname
                .trim_end_matches(&format!(".{}", SERVICE_TYPE)),
            &host_fullname,
            addresses
                .iter()
                .map(|a| a.to_string())
                .collect::<Vec<_>>()
                .join(","),
            port,
            properties,
        )
        .map_err(|e| AppError::Internal(format!("Failed to create service info: {}", e)))?;

        // Register the service
        self.daemon
            .register(service)
            .map_err(|e| AppError::Internal(format!("Failed to register mDNS service: {}", e)))?;

        *self.registered.write().await = true;

        info!(
            "Published mDNS service: {} on port {} with addresses: {:?}",
            self.service_fullname, port, addresses
        );

        Ok(())
    }

    /// Unpublishes the service from the network.
    pub async fn unpublish(&self) -> AppResult<()> {
        if !*self.registered.read().await {
            return Ok(());
        }

        self.daemon
            .unregister(&self.service_fullname)
            .map_err(|e| AppError::Internal(format!("Failed to unregister mDNS service: {}", e)))?;

        *self.registered.write().await = false;

        info!("Unpublished mDNS service: {}", self.service_fullname);

        Ok(())
    }

    /// Returns whether the service is currently registered.
    pub async fn is_registered(&self) -> bool {
        *self.registered.read().await
    }

    /// Returns the full service name.
    pub fn service_fullname(&self) -> &str {
        &self.service_fullname
    }

    /// Shuts down the mDNS daemon gracefully.
    pub fn shutdown(self) -> AppResult<()> {
        let _ = self
            .daemon
            .shutdown()
            .map_err(|e| AppError::Internal(format!("Failed to shutdown mDNS daemon: {}", e)))?;
        Ok(())
    }
}

impl Drop for MdnsPublisher {
    fn drop(&mut self) {
        // Try to unregister on drop (best effort)
        if let Err(e) = self.daemon.unregister(&self.service_fullname) {
            debug!("Failed to unregister mDNS service on drop: {}", e);
        }
    }
}

/// A discovered Cortex server on the network.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DiscoveredServer {
    /// The instance name of the service.
    pub name: String,
    /// The full service name (e.g., "cortex-8080._cortex._tcp.local.").
    pub fullname: String,
    /// The hostname of the server.
    pub host: String,
    /// The port the server is listening on.
    pub port: u16,
    /// IP addresses of the server.
    pub addresses: Vec<IpAddr>,
    /// Service properties/TXT records.
    pub properties: HashMap<String, String>,
    /// When this server was discovered (Unix timestamp in milliseconds).
    pub discovered_at: u64,
}

impl DiscoveredServer {
    /// Returns the first reachable URL for this server.
    pub fn url(&self) -> Option<String> {
        let path = self
            .properties
            .get("path")
            .map(|s| s.as_str())
            .unwrap_or("/");

        // Prefer IPv4 addresses
        for addr in &self.addresses {
            if addr.is_ipv4() {
                return Some(format!("http://{}:{}{}", addr, self.port, path));
            }
        }

        // Fall back to IPv6
        for addr in &self.addresses {
            if addr.is_ipv6() {
                return Some(format!("http://[{}]:{}{}", addr, self.port, path));
            }
        }

        // Fall back to hostname
        Some(format!(
            "http://{}:{}{}",
            self.host.trim_end_matches('.'),
            self.port,
            path
        ))
    }

    /// Returns the API version if available.
    pub fn api_version(&self) -> Option<&str> {
        self.properties.get("api").map(|s| s.as_str())
    }

    /// Returns the server version if available.
    pub fn version(&self) -> Option<&str> {
        self.properties.get("version").map(|s| s.as_str())
    }
}

/// mDNS discovery client for finding Cortex servers on the network.
pub struct MdnsDiscovery {
    daemon: ServiceDaemon,
}

impl MdnsDiscovery {
    /// Creates a new mDNS discovery client.
    pub fn new() -> AppResult<Self> {
        let daemon = ServiceDaemon::new()
            .map_err(|e| AppError::Internal(format!("Failed to create mDNS daemon: {}", e)))?;

        Ok(Self { daemon })
    }

    /// Discovers Cortex servers on the network.
    ///
    /// # Arguments
    /// * `timeout` - How long to wait for responses
    ///
    /// # Returns
    /// A vector of discovered servers, sorted by discovery time.
    pub async fn discover(&self, timeout: Duration) -> AppResult<Vec<DiscoveredServer>> {
        let receiver = self
            .daemon
            .browse(SERVICE_TYPE)
            .map_err(|e| AppError::Internal(format!("Failed to browse mDNS services: {}", e)))?;

        let mut servers: HashMap<String, DiscoveredServer> = HashMap::new();
        let deadline = Instant::now() + timeout;

        info!(
            "Starting mDNS discovery for {} (timeout: {:?})",
            SERVICE_TYPE, timeout
        );

        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());

            match tokio::time::timeout(
                remaining,
                tokio::task::spawn_blocking({
                    let receiver = receiver.clone();
                    move || receiver.recv_timeout(Duration::from_millis(100))
                }),
            )
            .await
            {
                Ok(Ok(Ok(event))) => match event {
                    ServiceEvent::ServiceResolved(info) => {
                        let fullname = info.get_fullname().to_string();
                        let instance_name = fullname
                            .trim_end_matches(SERVICE_TYPE)
                            .trim_end_matches('.')
                            .to_string();

                        let properties: HashMap<String, String> = info
                            .get_properties()
                            .iter()
                            .map(|p| (p.key().to_string(), p.val_str().to_string()))
                            .collect();

                        let server = DiscoveredServer {
                            name: instance_name,
                            fullname,
                            host: info.get_hostname().to_string(),
                            port: info.get_port(),
                            addresses: info.get_addresses().iter().copied().collect(),
                            properties,
                            discovered_at: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as u64,
                        };

                        info!(
                            "Discovered Cortex server: {} at {}:{} ({:?})",
                            server.name, server.host, server.port, server.addresses
                        );

                        servers.insert(server.fullname.clone(), server);
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => {
                        debug!("Service removed: {}", fullname);
                        servers.remove(&fullname);
                    }
                    ServiceEvent::SearchStarted(service_type) => {
                        debug!("mDNS search started for: {}", service_type);
                    }
                    ServiceEvent::SearchStopped(service_type) => {
                        debug!("mDNS search stopped for: {}", service_type);
                    }
                    _ => {}
                },
                Ok(Ok(Err(_))) => {
                    // Timeout on recv, continue
                    continue;
                }
                Ok(Err(e)) => {
                    warn!("mDNS discovery task error: {}", e);
                    break;
                }
                Err(_) => {
                    // Timeout elapsed
                    break;
                }
            }
        }

        // Stop browsing
        if let Err(e) = self.daemon.stop_browse(SERVICE_TYPE) {
            debug!("Failed to stop mDNS browse: {}", e);
        }

        let mut result: Vec<_> = servers.into_values().collect();
        result.sort_by_key(|s| s.discovered_at);

        info!("mDNS discovery complete: found {} server(s)", result.len());

        Ok(result)
    }

    /// Discovers servers continuously, calling the callback for each new server.
    ///
    /// This method runs until the returned handle is dropped or an error occurs.
    pub fn discover_continuous<F>(&self, callback: F) -> AppResult<MdnsDiscoveryHandle>
    where
        F: Fn(DiscoveredServer) + Send + Sync + 'static,
    {
        let receiver = self
            .daemon
            .browse(SERVICE_TYPE)
            .map_err(|e| AppError::Internal(format!("Failed to browse mDNS services: {}", e)))?;

        let (stop_tx, mut stop_rx) = tokio::sync::mpsc::channel::<()>(1);
        let callback = Arc::new(callback);

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop_rx.recv() => {
                        debug!("mDNS continuous discovery stopped");
                        break;
                    }
                    result = tokio::task::spawn_blocking({
                        let receiver = receiver.clone();
                        move || receiver.recv_timeout(Duration::from_millis(500))
                    }) => {
                        match result {
                            Ok(Ok(event)) => {
                                if let ServiceEvent::ServiceResolved(info) = event {
                                    let fullname = info.get_fullname().to_string();
                                    let instance_name = fullname
                                        .trim_end_matches(SERVICE_TYPE)
                                        .trim_end_matches('.')
                                        .to_string();

                                    let properties: HashMap<String, String> = info
                                        .get_properties()
                                        .iter()
                                        .map(|p| (p.key().to_string(), p.val_str().to_string()))
                                        .collect();

                                    let server = DiscoveredServer {
                                        name: instance_name,
                                        fullname,
                                        host: info.get_hostname().to_string(),
                                        port: info.get_port(),
                                        addresses: info.get_addresses().iter().copied().collect(),
                                        properties,
                                        discovered_at: std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_millis() as u64,
                                    };

                                    callback(server);
                                }
                            }
                            Ok(Err(_)) => continue, // Timeout, continue
                            Err(e) => {
                                error!("mDNS continuous discovery error: {}", e);
                                break;
                            }
                        }
                    }
                }
            }
        });

        Ok(MdnsDiscoveryHandle { stop_tx })
    }

    /// Shuts down the mDNS daemon.
    pub fn shutdown(self) -> AppResult<()> {
        let _ = self
            .daemon
            .shutdown()
            .map_err(|e| AppError::Internal(format!("Failed to shutdown mDNS daemon: {}", e)))?;
        Ok(())
    }
}

/// Handle for continuous discovery. Dropping this stops the discovery.
pub struct MdnsDiscoveryHandle {
    stop_tx: tokio::sync::mpsc::Sender<()>,
}

impl MdnsDiscoveryHandle {
    /// Stops the continuous discovery.
    pub async fn stop(self) {
        let _ = self.stop_tx.send(()).await;
    }
}

/// Gets the local IP addresses (non-loopback).
pub fn get_local_addresses() -> AppResult<Vec<IpAddr>> {
    let mut addresses = Vec::new();

    let interfaces = if_addrs::get_if_addrs()
        .map_err(|e| AppError::Internal(format!("Failed to get network interfaces: {}", e)))?;

    for iface in interfaces {
        let ip = iface.ip();

        // Skip loopback addresses
        if ip.is_loopback() {
            continue;
        }

        // Skip link-local addresses for IPv6
        if let IpAddr::V6(v6) = ip {
            // Skip link-local (fe80::/10)
            let segments = v6.segments();
            if segments[0] & 0xffc0 == 0xfe80 {
                continue;
            }
        }

        addresses.push(ip);
    }

    // Sort to prefer IPv4
    addresses.sort_by(|a, b| match (a, b) {
        (IpAddr::V4(_), IpAddr::V6(_)) => std::cmp::Ordering::Less,
        (IpAddr::V6(_), IpAddr::V4(_)) => std::cmp::Ordering::Greater,
        _ => std::cmp::Ordering::Equal,
    });

    Ok(addresses)
}

/// Shared mDNS state for the application.
pub struct MdnsState {
    publisher: Option<MdnsPublisher>,
    port: u16,
}

impl MdnsState {
    /// Creates a new mDNS state.
    pub fn new(port: u16) -> Self {
        Self {
            publisher: None,
            port,
        }
    }

    /// Starts mDNS publishing if enabled.
    pub async fn start(&mut self, service_name: Option<String>) -> AppResult<()> {
        let publisher = MdnsPublisher::new(self.port, service_name)?;
        publisher.publish(self.port).await?;
        self.publisher = Some(publisher);
        Ok(())
    }

    /// Stops mDNS publishing.
    pub async fn stop(&mut self) -> AppResult<()> {
        if let Some(publisher) = self.publisher.take() {
            publisher.unpublish().await?;
        }
        Ok(())
    }

    /// Returns whether mDNS is active.
    pub async fn is_active(&self) -> bool {
        if let Some(ref publisher) = self.publisher {
            publisher.is_registered().await
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_local_addresses() {
        let addresses = get_local_addresses().unwrap();
        // Should have at least one address on most systems
        // (unless running in a very isolated container)
        for addr in &addresses {
            assert!(!addr.is_loopback());
        }
    }

    #[test]
    fn test_discovered_server_url() {
        let server = DiscoveredServer {
            name: "test-server".to_string(),
            fullname: "test-server._cortex._tcp.local.".to_string(),
            host: "myhost.local.".to_string(),
            port: 8080,
            addresses: vec!["192.168.1.100".parse().unwrap(), "::1".parse().unwrap()],
            properties: [("path".to_string(), "/api".to_string())]
                .into_iter()
                .collect(),
            discovered_at: 0,
        };

        // Should prefer IPv4
        assert_eq!(
            server.url(),
            Some("http://192.168.1.100:8080/api".to_string())
        );
    }

    #[test]
    fn test_discovered_server_url_ipv6_only() {
        let server = DiscoveredServer {
            name: "test-server".to_string(),
            fullname: "test-server._cortex._tcp.local.".to_string(),
            host: "myhost.local.".to_string(),
            port: 8080,
            addresses: vec!["2001:db8::1".parse().unwrap()],
            properties: HashMap::new(),
            discovered_at: 0,
        };

        assert_eq!(server.url(), Some("http://[2001:db8::1]:8080/".to_string()));
    }

    #[test]
    fn test_discovered_server_url_hostname_fallback() {
        let server = DiscoveredServer {
            name: "test-server".to_string(),
            fullname: "test-server._cortex._tcp.local.".to_string(),
            host: "myhost.local.".to_string(),
            port: 8080,
            addresses: vec![],
            properties: HashMap::new(),
            discovered_at: 0,
        };

        assert_eq!(server.url(), Some("http://myhost.local:8080/".to_string()));
    }

    #[tokio::test]
    async fn test_mdns_state() {
        let state = MdnsState::new(8080);
        assert!(!state.is_active().await);
    }
}
