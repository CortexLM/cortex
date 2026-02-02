//! mDNS service browser for discovering Cortex servers.
//!
//! This module handles discovering Cortex servers on the local network
//! using mDNS/DNS-SD (Bonjour/Zeroconf).

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;

use flume::RecvTimeoutError;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo as MdnsServiceInfo};

use super::txt_keys;
use crate::error::{CortexError, Result};

/// A discovered Cortex service.
#[derive(Debug, Clone)]
pub struct DiscoveredService {
    /// The service instance name.
    pub name: String,
    /// The hostname of the service.
    pub host: String,
    /// The port the service is listening on.
    pub port: u16,
    /// The IP addresses of the service.
    pub addresses: Vec<IpAddr>,
    /// TXT records from the service.
    pub txt_records: HashMap<String, String>,
}

impl DiscoveredService {
    /// Create a discovered service from mdns-sd service info.
    fn from_mdns_info(info: &MdnsServiceInfo) -> Self {
        let txt_records: HashMap<String, String> = info
            .get_properties()
            .iter()
            .map(|prop| (prop.key().to_string(), prop.val_str().to_string()))
            .collect();

        Self {
            name: info.get_fullname().to_string(),
            host: info.get_hostname().to_string(),
            port: info.get_port(),
            addresses: info.get_addresses().iter().copied().collect(),
            txt_records,
        }
    }

    /// Get the Cortex version from TXT records.
    pub fn version(&self) -> Option<&str> {
        self.txt_records.get(txt_keys::VERSION).map(|s| s.as_str())
    }

    /// Get the machine hostname from TXT records.
    pub fn machine_hostname(&self) -> Option<&str> {
        self.txt_records.get(txt_keys::HOSTNAME).map(|s| s.as_str())
    }

    /// Get the username from TXT records.
    pub fn user(&self) -> Option<&str> {
        self.txt_records.get(txt_keys::USER).map(|s| s.as_str())
    }

    /// Get the server ID from TXT records.
    pub fn server_id(&self) -> Option<&str> {
        self.txt_records
            .get(txt_keys::SERVER_ID)
            .map(|s| s.as_str())
    }

    /// Get the first available IP address.
    pub fn first_address(&self) -> Option<&IpAddr> {
        self.addresses.first()
    }

    /// Get the connection URL for this service.
    pub fn url(&self) -> Option<String> {
        self.first_address()
            .map(|addr| format!("http://{addr}:{}", self.port))
    }

    /// Get a display-friendly name.
    pub fn display_name(&self) -> String {
        // Extract the instance name from the full service name
        // Format is typically: "Instance Name._cortex._tcp.local."
        self.name
            .split('.')
            .next()
            .unwrap_or(&self.name)
            .to_string()
    }
}

/// Service browser trait for discovering services.
pub trait ServiceBrowser {
    /// Browse for services on the network.
    fn browse(&self) -> impl std::future::Future<Output = Result<Vec<DiscoveredService>>> + Send;
}

/// mDNS browser for discovering Cortex servers.
pub struct MdnsBrowser {
    /// The service daemon.
    daemon: ServiceDaemon,
    /// Browse timeout.
    timeout: Duration,
}

impl MdnsBrowser {
    /// Create a new mDNS browser.
    pub async fn new() -> Result<Self> {
        Self::with_timeout(Duration::from_secs(3)).await
    }

    /// Create a new mDNS browser with a custom timeout.
    pub async fn with_timeout(timeout: Duration) -> Result<Self> {
        let daemon = ServiceDaemon::new()
            .map_err(|e| CortexError::MdnsError(format!("Failed to create mDNS daemon: {e}")))?;

        Ok(Self { daemon, timeout })
    }

    /// Browse for Cortex services on the network.
    pub async fn browse(&self) -> Result<Vec<DiscoveredService>> {
        // Use full service type with ".local." suffix as required by mDNS specification
        let service_type = super::SERVICE_TYPE;

        let receiver = self
            .daemon
            .browse(service_type)
            .map_err(|e| {
                // Provide a more user-friendly error message for common mDNS issues
                let msg = e.to_string();
                if msg.contains("must end with") {
                    CortexError::MdnsError(
                        "mDNS service discovery is not available. This may be due to network configuration or firewall settings.".to_string()
                    )
                } else {
                    CortexError::MdnsError(format!("Failed to start mDNS browse: {e}"))
                }
            })?;

        let mut services = Vec::new();
        let deadline = std::time::Instant::now() + self.timeout;

        // Collect services until timeout
        while std::time::Instant::now() < deadline {
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(event) => match event {
                    ServiceEvent::ServiceResolved(info) => {
                        tracing::debug!("Discovered service: {}", info.get_fullname());
                        services.push(DiscoveredService::from_mdns_info(&info));
                    }
                    ServiceEvent::ServiceFound(_, name) => {
                        tracing::trace!("Found service (not yet resolved): {name}");
                    }
                    ServiceEvent::ServiceRemoved(_, name) => {
                        tracing::trace!("Service removed: {name}");
                        // Remove from our list if present
                        services.retain(|s| !s.name.contains(&name));
                    }
                    ServiceEvent::SearchStarted(_) => {
                        tracing::trace!("mDNS search started");
                    }
                    ServiceEvent::SearchStopped(_) => {
                        tracing::trace!("mDNS search stopped");
                    }
                },
                Err(RecvTimeoutError::Timeout) => {
                    // Continue waiting until deadline
                }
                Err(RecvTimeoutError::Disconnected) => {
                    break;
                }
            }
        }

        // Stop browsing
        self.daemon.stop_browse(&service_type).ok();

        tracing::debug!("mDNS browse complete, found {} services", services.len());

        Ok(services)
    }

    /// Browse for a specific duration and return all found services.
    pub async fn browse_for(&self, duration: Duration) -> Result<Vec<DiscoveredService>> {
        // Use full service type with ".local." suffix as required by mDNS specification
        let service_type = super::SERVICE_TYPE;

        let receiver = self
            .daemon
            .browse(service_type)
            .map_err(|e| {
                // Provide a more user-friendly error message for common mDNS issues
                let msg = e.to_string();
                if msg.contains("must end with") {
                    CortexError::MdnsError(
                        "mDNS service discovery is not available. This may be due to network configuration or firewall settings.".to_string()
                    )
                } else {
                    CortexError::MdnsError(format!("Failed to start mDNS browse: {e}"))
                }
            })?;

        let mut services = Vec::new();
        let deadline = std::time::Instant::now() + duration;

        while std::time::Instant::now() < deadline {
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(ServiceEvent::ServiceResolved(info)) => {
                    services.push(DiscoveredService::from_mdns_info(&info));
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }

        self.daemon.stop_browse(&service_type).ok();
        Ok(services)
    }
}

impl ServiceBrowser for MdnsBrowser {
    async fn browse(&self) -> Result<Vec<DiscoveredService>> {
        self.browse().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discovered_service_display_name() {
        let service = DiscoveredService {
            name: "Cortex-test-12345678._cortex._tcp.local.".to_string(),
            host: "test.local.".to_string(),
            port: 3000,
            addresses: vec![],
            txt_records: HashMap::new(),
        };

        assert_eq!(service.display_name(), "Cortex-test-12345678");
    }

    #[test]
    fn test_discovered_service_url() {
        let service = DiscoveredService {
            name: "test".to_string(),
            host: "test.local.".to_string(),
            port: 3000,
            addresses: vec!["192.168.1.100".parse().unwrap()],
            txt_records: HashMap::new(),
        };

        assert_eq!(service.url(), Some("http://192.168.1.100:3000".to_string()));
    }
}
