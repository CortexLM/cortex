//! mDNS service advertiser for Cortex.
//!
//! This module handles advertising Cortex servers on the local network
//! using mDNS/DNS-SD (Bonjour/Zeroconf).

use std::collections::HashMap;

use mdns_sd::{ServiceDaemon, ServiceInfo as MdnsServiceInfo};

use super::{SERVICE_TYPE_SHORT, build_txt_records, txt_keys};
use crate::error::{CortexError, Result};

/// Information about a Cortex service.
#[derive(Debug, Clone)]
pub struct ServiceInfo {
    /// The port the service is listening on.
    pub port: u16,
    /// The friendly name of the service.
    pub name: String,
    /// TXT records for the service.
    pub txt_records: HashMap<String, String>,
    /// The full service name.
    pub full_name: String,
}

impl ServiceInfo {
    /// Create new service info.
    pub fn new(port: u16, name: &str) -> Result<Self> {
        let txt_records = build_txt_records();
        let full_name = format!("{}.{}.local.", name, SERVICE_TYPE_SHORT);

        Ok(Self {
            port,
            name: name.to_string(),
            txt_records,
            full_name,
        })
    }

    /// Get the version from TXT records.
    pub fn version(&self) -> Option<&str> {
        self.txt_records.get(txt_keys::VERSION).map(|s| s.as_str())
    }

    /// Get the hostname from TXT records.
    pub fn hostname(&self) -> Option<&str> {
        self.txt_records.get(txt_keys::HOSTNAME).map(|s| s.as_str())
    }

    /// Get the user from TXT records.
    pub fn user(&self) -> Option<&str> {
        self.txt_records.get(txt_keys::USER).map(|s| s.as_str())
    }
}

/// mDNS advertiser for broadcasting Cortex services.
pub struct MdnsAdvertiser {
    /// The service daemon.
    daemon: ServiceDaemon,
    /// Service info being advertised.
    service_info: ServiceInfo,
    /// Whether the service is currently registered.
    registered: bool,
}

impl MdnsAdvertiser {
    /// Create a new mDNS advertiser.
    pub async fn new(service_info: ServiceInfo) -> Result<Self> {
        let daemon = ServiceDaemon::new()
            .map_err(|e| CortexError::MdnsError(format!("Failed to create mDNS daemon: {e}")))?;

        Ok(Self {
            daemon,
            service_info,
            registered: false,
        })
    }

    /// Start advertising the service.
    pub async fn start(&self) -> Result<()> {
        if self.registered {
            return Ok(());
        }

        // Get the hostname
        let hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "cortex-server".to_string());

        // Build the service name (instance name)
        let instance_name = format!(
            "Cortex-{}-{}",
            self.service_info.name,
            &self
                .service_info
                .txt_records
                .get(txt_keys::SERVER_ID)
                .map(|s| s[..8].to_string())
                .unwrap_or_else(|| "unknown".to_string())
        );

        // Convert TXT records to the format expected by mdns-sd
        let properties: Vec<(&str, &str)> = self
            .service_info
            .txt_records
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        // Create the mDNS service info
        let service_type = super::SERVICE_TYPE;
        let mdns_service = MdnsServiceInfo::new(
            &service_type,
            &instance_name,
            &format!("{hostname}.local."),
            "", // Let mdns-sd determine the IP
            self.service_info.port,
            properties.as_slice(),
        )
        .map_err(|e| CortexError::MdnsError(format!("Failed to create service info: {e}")))?;

        // Register the service
        self.daemon
            .register(mdns_service)
            .map_err(|e| CortexError::MdnsError(format!("Failed to register mDNS service: {e}")))?;

        tracing::debug!(
            "Registered mDNS service '{}' on port {}",
            instance_name,
            self.service_info.port
        );

        Ok(())
    }

    /// Stop advertising the service.
    pub async fn stop(&self) -> Result<()> {
        // The service is automatically unregistered when the daemon is dropped
        // For explicit unregistration, we would need to track the full service name
        tracing::debug!("mDNS advertiser stopped");
        Ok(())
    }

    /// Get the service info.
    pub fn service_info(&self) -> &ServiceInfo {
        &self.service_info
    }
}

impl Drop for MdnsAdvertiser {
    fn drop(&mut self) {
        // mdns-sd automatically cleans up when daemon is dropped
        tracing::debug!("mDNS advertiser dropped, service unregistered");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_info_new() {
        let info = ServiceInfo::new(3000, "test-server").unwrap();
        assert_eq!(info.port, 3000);
        assert_eq!(info.name, "test-server");
        assert!(info.version().is_some());
    }

    #[test]
    fn test_service_info_full_name() {
        let info = ServiceInfo::new(8080, "my-cortex").unwrap();
        assert!(info.full_name.contains("_cortex._tcp"));
    }
}
