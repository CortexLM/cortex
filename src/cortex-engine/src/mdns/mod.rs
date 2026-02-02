//! mDNS (Bonjour/Zeroconf) service discovery for Cortex.
//!
//! This module provides local network discovery of Cortex servers using mDNS/DNS-SD.
//! It allows:
//! - Advertising Cortex servers on the local network
//! - Discovering other Cortex servers on the local network
//!
//! Service type: `_cortex._tcp.local`

mod advertiser;
mod browser;

pub use advertiser::{MdnsAdvertiser, ServiceInfo};
pub use browser::{DiscoveredService, MdnsBrowser, ServiceBrowser};

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// The mDNS service type for Cortex servers.
pub const SERVICE_TYPE: &str = "_cortex._tcp.local.";

/// The mDNS service type without trailing dot.
pub const SERVICE_TYPE_SHORT: &str = "_cortex._tcp";

/// Default mDNS domain.
pub const DEFAULT_DOMAIN: &str = "local";

/// Cortex mDNS service for advertising and discovering services.
#[derive(Clone)]
pub struct MdnsService {
    /// Advertiser for broadcasting our service.
    advertiser: Option<Arc<RwLock<MdnsAdvertiser>>>,
    /// Browser for discovering other services.
    browser: Option<Arc<RwLock<MdnsBrowser>>>,
    /// Local service info if advertising.
    local_service: Option<ServiceInfo>,
}

impl MdnsService {
    /// Create a new mDNS service.
    pub fn new() -> Self {
        Self {
            advertiser: None,
            browser: None,
            local_service: None,
        }
    }

    /// Advertise a Cortex server on the local network.
    ///
    /// # Arguments
    /// * `port` - The port the server is listening on
    /// * `name` - A friendly name for the service (e.g., hostname or custom name)
    ///
    /// # Returns
    /// The service info that was advertised.
    pub async fn advertise(&mut self, port: u16, name: &str) -> crate::Result<ServiceInfo> {
        let service_info = ServiceInfo::new(port, name)?;

        let advertiser = MdnsAdvertiser::new(service_info.clone()).await?;
        advertiser.start().await?;

        self.advertiser = Some(Arc::new(RwLock::new(advertiser)));
        self.local_service = Some(service_info.clone());

        tracing::info!(
            "Advertising Cortex server '{}' on port {} via mDNS",
            name,
            port
        );

        Ok(service_info)
    }

    /// Stop advertising the service.
    pub async fn stop_advertising(&mut self) -> crate::Result<()> {
        if let Some(advertiser) = self.advertiser.take() {
            let advertiser = advertiser.write().await;
            advertiser.stop().await?;
            tracing::info!("Stopped mDNS advertising");
        }
        self.local_service = None;
        Ok(())
    }

    /// Discover Cortex servers on the local network.
    ///
    /// # Returns
    /// A list of discovered services.
    pub async fn discover(&mut self) -> crate::Result<Vec<DiscoveredService>> {
        let browser = if let Some(browser) = &self.browser {
            browser.clone()
        } else {
            let new_browser = MdnsBrowser::new().await?;
            let arc_browser = Arc::new(RwLock::new(new_browser));
            self.browser = Some(arc_browser.clone());
            arc_browser
        };

        let browser = browser.read().await;
        browser.browse().await
    }

    /// Get the local service info if advertising.
    pub fn local_service(&self) -> Option<&ServiceInfo> {
        self.local_service.as_ref()
    }

    /// Check if currently advertising.
    pub fn is_advertising(&self) -> bool {
        self.advertiser.is_some()
    }
}

impl Default for MdnsService {
    fn default() -> Self {
        Self::new()
    }
}

/// TXT record keys for Cortex mDNS services.
pub mod txt_keys {
    /// The Cortex version.
    pub const VERSION: &str = "version";
    /// The machine hostname.
    pub const HOSTNAME: &str = "hostname";
    /// The username.
    pub const USER: &str = "user";
    /// The server ID.
    pub const SERVER_ID: &str = "server_id";
    /// Whether authentication is required.
    pub const AUTH_REQUIRED: &str = "auth_required";
}

/// Build TXT records for a Cortex service.
pub fn build_txt_records() -> HashMap<String, String> {
    let mut records = HashMap::new();

    // Add version
    records.insert(
        txt_keys::VERSION.to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
    );

    // Add hostname
    if let Ok(hostname) = hostname::get() {
        records.insert(
            txt_keys::HOSTNAME.to_string(),
            hostname.to_string_lossy().to_string(),
        );
    }

    // Add username
    if let Ok(user) = std::env::var("USER").or_else(|_| std::env::var("USERNAME")) {
        records.insert(txt_keys::USER.to_string(), user);
    }

    // Add server ID
    records.insert(
        txt_keys::SERVER_ID.to_string(),
        uuid::Uuid::new_v4().to_string(),
    );

    records
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_txt_records() {
        let records = build_txt_records();
        assert!(records.contains_key(txt_keys::VERSION));
        assert!(records.contains_key(txt_keys::SERVER_ID));
    }

    #[test]
    fn test_mdns_service_new() {
        let service = MdnsService::new();
        assert!(!service.is_advertising());
        assert!(service.local_service().is_none());
    }
}
