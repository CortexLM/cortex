//! Plugin registry for managing loaded plugins.
//!
//! This module provides both local plugin registry management and remote
//! registry discovery, including plugin index fetching and update checking.
//!
//! # Security
//!
//! This module implements several security measures:
//! - SSRF protection: URLs are validated before downloading to block private IPs
//!   and dangerous protocols
//! - Directory traversal protection: Plugin IDs are validated to prevent path
//!   traversal attacks via "../" sequences

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::plugin::{Plugin, PluginHandle, PluginInfo, PluginState, PluginStats, PluginStatus};
use crate::signing::PluginSigner;
use crate::{PluginError, Result};

/// Remote registry configuration.
///
/// Represents a remote plugin registry that can be queried for
/// available plugins and updates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteRegistry {
    /// Base URL of the registry (e.g., "https://plugins.cortex.dev")
    pub url: String,
    /// Human-readable name of the registry
    pub name: String,
    /// Whether this registry is enabled for queries
    pub enabled: bool,
}

impl RemoteRegistry {
    /// Create a new remote registry configuration.
    pub fn new(url: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            name: name.into(),
            enabled: true,
        }
    }

    /// Create a disabled remote registry configuration.
    pub fn new_disabled(url: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            name: name.into(),
            enabled: false,
        }
    }
}

/// Plugin index entry from a remote registry.
///
/// Contains metadata about an available plugin including
/// download URL, checksum, and optional signature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginIndexEntry {
    /// Unique plugin identifier
    pub id: String,
    /// Human-readable plugin name
    pub name: String,
    /// Plugin version (semver)
    pub version: String,
    /// Plugin description
    pub description: String,
    /// URL to download the plugin package
    pub download_url: String,
    /// SHA256 checksum of the plugin package (hex-encoded)
    pub checksum: String,
    /// Optional ed25519 signature (hex-encoded)
    pub signature: Option<String>,
    /// When this entry was last updated
    pub updated_at: DateTime<Utc>,
}

impl PluginIndexEntry {
    /// Check if this entry has a signature.
    pub fn is_signed(&self) -> bool {
        self.signature.is_some()
    }
}

/// Remote plugin index response.
///
/// The format expected from remote registry API endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginIndex {
    /// List of available plugins
    pub plugins: Vec<PluginIndexEntry>,
    /// When the index was generated
    pub generated_at: DateTime<Utc>,
}

/// Registry for loaded plugins.
pub struct PluginRegistry {
    /// Loaded plugins by ID
    plugins: RwLock<HashMap<String, PluginHandle>>,

    /// Plugin statistics
    stats: RwLock<HashMap<String, PluginStats>>,

    /// Configured remote registries
    remote_registries: RwLock<Vec<RemoteRegistry>>,

    /// Cached plugin index from remote registries
    cached_index: RwLock<HashMap<String, Vec<PluginIndexEntry>>>,

    /// HTTP client for remote operations
    http_client: reqwest::Client,

    /// Plugin signer for signature verification
    signer: RwLock<PluginSigner>,
}

impl PluginRegistry {
    /// Create a new plugin registry.
    pub fn new() -> Self {
        let http_client = reqwest::Client::builder()
            .user_agent(format!("cortex-plugins/{}", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            plugins: RwLock::new(HashMap::new()),
            stats: RwLock::new(HashMap::new()),
            remote_registries: RwLock::new(Vec::new()),
            cached_index: RwLock::new(HashMap::new()),
            http_client,
            signer: RwLock::new(PluginSigner::new()),
        }
    }

    /// Add a remote registry.
    pub async fn add_remote_registry(&self, registry: RemoteRegistry) {
        let mut registries = self.remote_registries.write().await;

        // Check if already exists (by URL)
        if !registries.iter().any(|r| r.url == registry.url) {
            tracing::info!(
                "Adding remote registry: {} ({})",
                registry.name,
                registry.url
            );
            registries.push(registry);
        }
    }

    /// Remove a remote registry by URL.
    pub async fn remove_remote_registry(&self, url: &str) {
        let mut registries = self.remote_registries.write().await;
        registries.retain(|r| r.url != url);
    }

    /// List configured remote registries.
    pub async fn list_remote_registries(&self) -> Vec<RemoteRegistry> {
        self.remote_registries.read().await.clone()
    }

    /// Add a trusted signing key.
    pub async fn add_trusted_key(&self, key_bytes: &[u8]) -> Result<()> {
        let mut signer = self.signer.write().await;
        signer.add_trusted_key(key_bytes)
    }

    /// Add a trusted signing key from hex string.
    pub async fn add_trusted_key_hex(&self, hex_key: &str) -> Result<()> {
        let mut signer = self.signer.write().await;
        signer.add_trusted_key_hex(hex_key)
    }

    /// Fetch plugin index from a remote registry.
    ///
    /// The index is cached for subsequent queries.
    pub async fn fetch_remote_index(
        &self,
        registry: &RemoteRegistry,
    ) -> Result<Vec<PluginIndexEntry>> {
        if !registry.enabled {
            tracing::debug!("Registry {} is disabled, skipping", registry.name);
            return Ok(Vec::new());
        }

        let index_url = format!(
            "{}/api/v1/plugins/index",
            registry.url.trim_end_matches('/')
        );
        tracing::debug!("Fetching plugin index from: {}", index_url);

        let response = self.http_client.get(&index_url).send().await.map_err(|e| {
            tracing::warn!("Failed to fetch index from {}: {}", registry.name, e);
            PluginError::NetworkError(format!(
                "Failed to fetch index from {}: {}",
                registry.name, e
            ))
        })?;

        if !response.status().is_success() {
            let status = response.status();
            tracing::warn!(
                "Registry {} returned error status: {}",
                registry.name,
                status
            );
            return Err(PluginError::RegistryError(format!(
                "Registry {} returned status {}",
                registry.name, status
            )));
        }

        let index: PluginIndex = response.json().await.map_err(|e| {
            tracing::warn!("Failed to parse index from {}: {}", registry.name, e);
            PluginError::RegistryError(format!(
                "Failed to parse index from {}: {}",
                registry.name, e
            ))
        })?;

        // Cache the index
        {
            let mut cached = self.cached_index.write().await;
            cached.insert(registry.url.clone(), index.plugins.clone());
        }

        tracing::info!(
            "Fetched {} plugins from registry {}",
            index.plugins.len(),
            registry.name
        );

        Ok(index.plugins)
    }

    /// Fetch indices from all enabled remote registries.
    pub async fn fetch_all_remote_indices(&self) -> Vec<PluginIndexEntry> {
        let registries = self.remote_registries.read().await.clone();
        let mut all_entries = Vec::new();

        for registry in registries {
            match self.fetch_remote_index(&registry).await {
                Ok(entries) => all_entries.extend(entries),
                Err(e) => {
                    tracing::warn!("Failed to fetch from {}: {}", registry.name, e);
                    // Continue with other registries
                }
            }
        }

        all_entries
    }

    /// Search for plugins by query string.
    ///
    /// Searches plugin ID, name, and description across all cached indices.
    pub async fn search(&self, query: &str) -> Result<Vec<PluginIndexEntry>> {
        let query_lower = query.to_lowercase();
        let cached = self.cached_index.read().await;

        let mut results: Vec<PluginIndexEntry> = cached
            .values()
            .flatten()
            .filter(|entry| {
                entry.id.to_lowercase().contains(&query_lower)
                    || entry.name.to_lowercase().contains(&query_lower)
                    || entry.description.to_lowercase().contains(&query_lower)
            })
            .cloned()
            .collect();

        // Sort by relevance (exact ID match first, then name match, then description)
        results.sort_by(|a, b| {
            let a_id_match = a.id.to_lowercase() == query_lower;
            let b_id_match = b.id.to_lowercase() == query_lower;

            if a_id_match && !b_id_match {
                return std::cmp::Ordering::Less;
            }
            if b_id_match && !a_id_match {
                return std::cmp::Ordering::Greater;
            }

            let a_name_match = a.name.to_lowercase().contains(&query_lower);
            let b_name_match = b.name.to_lowercase().contains(&query_lower);

            if a_name_match && !b_name_match {
                return std::cmp::Ordering::Less;
            }
            if b_name_match && !a_name_match {
                return std::cmp::Ordering::Greater;
            }

            a.name.cmp(&b.name)
        });

        Ok(results)
    }

    /// Check for updates to installed plugins.
    ///
    /// Returns a list of tuples: (plugin_id, current_version, available_version).
    pub async fn check_updates(&self) -> Result<Vec<(String, String, String)>> {
        let plugins = self.plugins.read().await;
        let cached = self.cached_index.read().await;

        let mut updates = Vec::new();

        for (id, handle) in plugins.iter() {
            let info = handle.info().await;
            let current_version = &info.version;

            // Look for this plugin in cached indices
            for entries in cached.values() {
                if let Some(entry) = entries.iter().find(|e| e.id == *id) {
                    // Compare versions using semver
                    if let (Ok(current), Ok(available)) = (
                        semver::Version::parse(current_version),
                        semver::Version::parse(&entry.version),
                    ) {
                        if available > current {
                            updates.push((
                                id.clone(),
                                current_version.clone(),
                                entry.version.clone(),
                            ));
                        }
                    }
                    break;
                }
            }
        }

        if !updates.is_empty() {
            tracing::info!("Found {} plugin updates available", updates.len());
        }

        Ok(updates)
    }

    /// Download a plugin from the remote registry.
    ///
    /// Downloads the plugin, verifies checksum and optional signature,
    /// then saves to the target directory.
    ///
    /// # Security
    ///
    /// This function implements several security checks:
    /// - SSRF protection: Validates the download URL to block private IPs and dangerous ports
    /// - Directory traversal protection: Validates plugin ID to prevent "../" path traversal
    /// - Checksum verification: Ensures downloaded content matches expected hash
    /// - Signature verification: Optionally verifies plugin signature if trusted keys are configured
    pub async fn download_plugin(
        &self,
        entry: &PluginIndexEntry,
        target_dir: &Path,
    ) -> Result<PathBuf> {
        // Security: Validate plugin ID to prevent directory traversal attacks
        // Plugin IDs must not contain path separators or ".." sequences
        if entry.id.contains("..") || entry.id.contains('/') || entry.id.contains('\\') {
            return Err(PluginError::validation_error(
                "plugin_id",
                "Plugin ID contains invalid characters (path separators or '..')",
            ));
        }

        // Security: Validate URL to prevent SSRF attacks
        // Block requests to private IPs, localhost, and dangerous ports
        Self::validate_download_url(&entry.download_url)?;

        tracing::info!(
            "Downloading plugin {} v{} from {}",
            entry.id,
            entry.version,
            entry.download_url
        );

        // Download the plugin package
        let response = self
            .http_client
            .get(&entry.download_url)
            .send()
            .await
            .map_err(|e| {
                PluginError::NetworkError(format!("Failed to download plugin {}: {}", entry.id, e))
            })?;

        if !response.status().is_success() {
            return Err(PluginError::NetworkError(format!(
                "Failed to download plugin {}: HTTP {}",
                entry.id,
                response.status()
            )));
        }

        let bytes = response.bytes().await.map_err(|e| {
            PluginError::NetworkError(format!(
                "Failed to read plugin data for {}: {}",
                entry.id, e
            ))
        })?;

        // Verify checksum
        let actual_checksum = PluginSigner::compute_checksum(&bytes);
        if !actual_checksum.eq_ignore_ascii_case(&entry.checksum) {
            return Err(PluginError::checksum_mismatch(
                &entry.id,
                &entry.checksum,
                &actual_checksum,
            ));
        }
        tracing::debug!("Checksum verified for plugin {}", entry.id);

        // Verify signature if present
        if let Some(ref signature) = entry.signature {
            let signer = self.signer.read().await;
            if signer.has_trusted_keys() {
                match signer.verify_plugin_hex(&bytes, signature) {
                    Ok(true) => {
                        tracing::debug!("Signature verified for plugin {}", entry.id);
                    }
                    Ok(false) => {
                        return Err(PluginError::SignatureError(format!(
                            "Plugin {} signature verification failed - not signed by trusted key",
                            entry.id
                        )));
                    }
                    Err(e) => {
                        return Err(PluginError::SignatureError(format!(
                            "Plugin {} signature verification error: {}",
                            entry.id, e
                        )));
                    }
                }
            } else {
                tracing::warn!(
                    "Plugin {} is signed but no trusted keys configured - skipping signature verification",
                    entry.id
                );
            }
        } else {
            tracing::warn!("Plugin {} is not signed", entry.id);
        }

        // Create plugin directory (safe now that plugin ID is validated)
        let plugin_dir = target_dir.join(&entry.id);
        tokio::fs::create_dir_all(&plugin_dir).await?;

        // Save the plugin file (assuming it's a WASM file)
        let plugin_path = plugin_dir.join("plugin.wasm");
        tokio::fs::write(&plugin_path, &bytes).await?;

        tracing::info!(
            "Downloaded plugin {} v{} to {}",
            entry.id,
            entry.version,
            plugin_path.display()
        );

        Ok(plugin_path)
    }

    /// Validate a URL for SSRF (Server-Side Request Forgery) protection.
    ///
    /// Blocks requests to:
    /// - Private IP ranges (10.x.x.x, 172.16-31.x.x, 192.168.x.x)
    /// - Localhost (127.x.x.x, ::1)
    /// - Link-local addresses (169.254.x.x, fe80::/10)
    /// - Non-HTTPS protocols (except for local development)
    /// - Cloud metadata endpoints (169.254.169.254)
    fn validate_download_url(url: &str) -> Result<()> {
        let parsed = url::Url::parse(url).map_err(|e| {
            PluginError::validation_error("download_url", format!("Invalid URL: {}", e))
        })?;

        // Only allow HTTPS for security (block file://, ftp://, etc.)
        match parsed.scheme() {
            "https" => {} // OK
            "http" => {
                // Allow HTTP only for local development with explicit localhost domains
                // But block it for IP addresses to prevent SSRF
                if let Some(host) = parsed.host_str() {
                    if !host.ends_with(".localhost") && host != "localhost" {
                        return Err(PluginError::validation_error(
                            "download_url",
                            "HTTP URLs are only allowed for localhost; use HTTPS for remote URLs",
                        ));
                    }
                }
            }
            other => {
                return Err(PluginError::validation_error(
                    "download_url",
                    format!("Unsupported URL scheme '{}'; only HTTPS is allowed", other),
                ));
            }
        }

        // Check for dangerous hosts
        if let Some(host) = parsed.host() {
            match host {
                url::Host::Ipv4(ip) => {
                    if Self::is_private_ipv4(ip) {
                        return Err(PluginError::validation_error(
                            "download_url",
                            format!("Download URL points to private/internal IP address: {}", ip),
                        ));
                    }
                }
                url::Host::Ipv6(ip) => {
                    if Self::is_private_ipv6(ip) {
                        return Err(PluginError::validation_error(
                            "download_url",
                            format!(
                                "Download URL points to private/internal IPv6 address: {}",
                                ip
                            ),
                        ));
                    }
                }
                url::Host::Domain(domain) => {
                    // Block common internal/metadata domains
                    let lower_domain = domain.to_lowercase();
                    if lower_domain == "localhost"
                        || lower_domain.ends_with(".local")
                        || lower_domain.ends_with(".internal")
                        || lower_domain == "metadata.google.internal"
                        || lower_domain.contains("169.254.169.254")
                    {
                        return Err(PluginError::validation_error(
                            "download_url",
                            format!(
                                "Download URL points to internal/metadata domain: {}",
                                domain
                            ),
                        ));
                    }
                }
            }
        }

        // Block dangerous ports commonly used for internal services
        if let Some(port) = parsed.port() {
            const DANGEROUS_PORTS: &[u16] = &[
                22,    // SSH
                23,    // Telnet
                25,    // SMTP
                135,   // RPC
                137,   // NetBIOS
                138,   // NetBIOS
                139,   // NetBIOS
                445,   // SMB
                1433,  // MSSQL
                1521,  // Oracle
                3306,  // MySQL
                3389,  // RDP
                5432,  // PostgreSQL
                5900,  // VNC
                6379,  // Redis
                8080,  // Common proxy
                8443,  // Alt HTTPS
                9200,  // Elasticsearch
                27017, // MongoDB
            ];

            if DANGEROUS_PORTS.contains(&port) {
                return Err(PluginError::validation_error(
                    "download_url",
                    format!("Download URL uses a potentially dangerous port: {}", port),
                ));
            }
        }

        Ok(())
    }

    /// Check if an IPv4 address is private/internal.
    fn is_private_ipv4(ip: std::net::Ipv4Addr) -> bool {
        // Loopback (127.0.0.0/8)
        if ip.is_loopback() {
            return true;
        }
        // Private ranges (RFC 1918)
        if ip.is_private() {
            return true;
        }
        // Link-local (169.254.0.0/16) - includes AWS/GCP/Azure metadata endpoint
        if ip.is_link_local() {
            return true;
        }
        // Broadcast
        if ip.is_broadcast() {
            return true;
        }
        // Documentation ranges
        if ip.is_documentation() {
            return true;
        }
        // Unspecified (0.0.0.0)
        if ip.is_unspecified() {
            return true;
        }
        // Shared address space (100.64.0.0/10) - RFC 6598 (carrier-grade NAT)
        let octets = ip.octets();
        if octets[0] == 100 && (octets[1] >= 64 && octets[1] <= 127) {
            return true;
        }
        // Localhost alternate (0.0.0.0)
        if octets[0] == 0 {
            return true;
        }

        false
    }

    /// Check if an IPv6 address is private/internal.
    fn is_private_ipv6(ip: std::net::Ipv6Addr) -> bool {
        // Loopback (::1)
        if ip.is_loopback() {
            return true;
        }
        // Unspecified (::)
        if ip.is_unspecified() {
            return true;
        }
        // Check if it's an IPv4-mapped address and validate the IPv4 part
        if let Some(ipv4) = ip.to_ipv4_mapped() {
            return Self::is_private_ipv4(ipv4);
        }
        // Link-local (fe80::/10)
        let segments = ip.segments();
        if segments[0] & 0xffc0 == 0xfe80 {
            return true;
        }
        // Unique local address (fc00::/7)
        if segments[0] & 0xfe00 == 0xfc00 {
            return true;
        }
        // Site-local (deprecated, fec0::/10)
        if segments[0] & 0xffc0 == 0xfec0 {
            return true;
        }

        false
    }

    /// Get a plugin index entry by ID from the cached index.
    pub async fn get_index_entry(&self, plugin_id: &str) -> Option<PluginIndexEntry> {
        let cached = self.cached_index.read().await;

        for entries in cached.values() {
            if let Some(entry) = entries.iter().find(|e| e.id == plugin_id) {
                return Some(entry.clone());
            }
        }

        None
    }

    /// Clear the cached plugin index.
    pub async fn clear_index_cache(&self) {
        let mut cached = self.cached_index.write().await;
        cached.clear();
        tracing::debug!("Cleared plugin index cache");
    }

    /// Register a plugin.
    pub async fn register(&self, plugin: Box<dyn Plugin>) -> Result<()> {
        let info = plugin.info().clone();
        let id = info.id.clone();

        // Use entry API to atomically check-and-insert within a single write lock
        // to prevent TOCTOU race conditions where multiple concurrent registrations
        // could both pass the contains_key check before either inserts
        let handle = PluginHandle::new(plugin);
        {
            let mut plugins = self.plugins.write().await;
            use std::collections::hash_map::Entry;
            match plugins.entry(id.clone()) {
                Entry::Occupied(_) => {
                    return Err(PluginError::AlreadyExists(id));
                }
                Entry::Vacant(entry) => {
                    entry.insert(handle);
                }
            }
        }

        {
            let mut stats = self.stats.write().await;
            stats.insert(id.clone(), PluginStats::default());
        }

        tracing::info!("Registered plugin: {} v{}", info.name, info.version);
        Ok(())
    }

    /// Unregister a plugin.
    pub async fn unregister(&self, id: &str) -> Result<()> {
        let handle = {
            let mut plugins = self.plugins.write().await;
            plugins.remove(id)
        };

        if let Some(handle) = handle {
            // Shutdown the plugin
            let mut plugin = handle.write().await;
            plugin.shutdown().await?;

            tracing::info!("Unregistered plugin: {}", id);
        }

        {
            let mut stats = self.stats.write().await;
            stats.remove(id);
        }

        Ok(())
    }

    /// Get a plugin by ID.
    pub async fn get(&self, id: &str) -> Option<PluginHandle> {
        self.plugins.read().await.get(id).cloned()
    }

    /// Check if a plugin is registered.
    pub async fn is_registered(&self, id: &str) -> bool {
        self.plugins.read().await.contains_key(id)
    }

    /// List all registered plugins.
    pub async fn list(&self) -> Vec<PluginInfo> {
        let plugins = self.plugins.read().await;
        let mut infos = Vec::new();

        for handle in plugins.values() {
            infos.push(handle.info().await);
        }

        infos.sort_by(|a, b| a.name.cmp(&b.name));
        infos
    }

    /// List plugins with detailed status.
    pub async fn list_status(&self) -> Vec<PluginStatus> {
        let plugins = self.plugins.read().await;
        let stats = self.stats.read().await;
        let mut statuses = Vec::new();

        for (id, handle) in plugins.iter() {
            let info = handle.info().await;
            let state = handle.state().await;
            let plugin_stats = stats.get(id).cloned().unwrap_or_default();

            statuses.push(PluginStatus {
                info,
                state,
                error: None,
                last_activity: None,
                stats: plugin_stats,
            });
        }

        statuses.sort_by(|a, b| a.info.name.cmp(&b.info.name));
        statuses
    }

    /// Get plugin count.
    pub async fn count(&self) -> usize {
        self.plugins.read().await.len()
    }

    /// Initialize all plugins.
    pub async fn init_all(&self) -> Vec<(String, Result<()>)> {
        let plugins = self.plugins.read().await;
        let mut results = Vec::new();

        for (id, handle) in plugins.iter() {
            let mut plugin = handle.write().await;
            let result = plugin.init().await;

            if let Err(ref e) = result {
                tracing::warn!("Failed to initialize plugin {}: {}", id, e);
            }

            results.push((id.clone(), result));
        }

        results
    }

    /// Shutdown all plugins.
    pub async fn shutdown_all(&self) -> Vec<(String, Result<()>)> {
        let plugins = self.plugins.read().await;
        let mut results = Vec::new();

        for (id, handle) in plugins.iter() {
            let mut plugin = handle.write().await;
            let result = plugin.shutdown().await;

            if let Err(ref e) = result {
                tracing::warn!("Failed to shutdown plugin {}: {}", id, e);
            }

            results.push((id.clone(), result));
        }

        results
    }

    /// Update statistics for a plugin.
    pub async fn update_stats<F>(&self, id: &str, f: F)
    where
        F: FnOnce(&mut PluginStats),
    {
        let mut stats = self.stats.write().await;
        if let Some(plugin_stats) = stats.get_mut(id) {
            f(plugin_stats);
        }
    }

    /// Get statistics for a plugin.
    pub async fn get_stats(&self, id: &str) -> Option<PluginStats> {
        self.stats.read().await.get(id).cloned()
    }

    /// Get IDs of all active plugins.
    pub async fn active_plugin_ids(&self) -> Vec<String> {
        let plugins = self.plugins.read().await;
        let mut ids = Vec::new();

        for (id, handle) in plugins.iter() {
            if handle.state().await == PluginState::Active {
                ids.push(id.clone());
            }
        }

        ids
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{PluginManifest, PluginMetadata};
    use std::path::PathBuf;

    // Mock plugin for testing
    struct MockPlugin {
        info: PluginInfo,
        manifest: PluginManifest,
        state: PluginState,
    }

    impl MockPlugin {
        fn new(id: &str) -> Self {
            let manifest = PluginManifest {
                plugin: PluginMetadata {
                    id: id.to_string(),
                    name: format!("Test Plugin {}", id),
                    version: "1.0.0".to_string(),
                    description: "A test plugin".to_string(),
                    authors: vec![],
                    homepage: None,
                    license: None,
                    min_cortex_version: None,
                    keywords: vec![],
                    icon: None,
                },
                capabilities: vec![],
                permissions: vec![],
                dependencies: vec![],
                commands: vec![],
                hooks: vec![],
                config: HashMap::new(),
                wasm: Default::default(),
            };

            let info = PluginInfo::from_manifest(&manifest, PathBuf::from("/tmp"));

            Self {
                info,
                manifest,
                state: PluginState::Loaded,
            }
        }
    }

    #[async_trait::async_trait]
    impl Plugin for MockPlugin {
        fn info(&self) -> &PluginInfo {
            &self.info
        }

        fn manifest(&self) -> &PluginManifest {
            &self.manifest
        }

        fn state(&self) -> PluginState {
            self.state
        }

        async fn init(&mut self) -> Result<()> {
            self.state = PluginState::Active;
            Ok(())
        }

        async fn shutdown(&mut self) -> Result<()> {
            self.state = PluginState::Unloaded;
            Ok(())
        }

        async fn execute_command(
            &self,
            name: &str,
            _args: Vec<String>,
            _ctx: &crate::PluginContext,
        ) -> Result<String> {
            Ok(format!("Mock command: {}", name))
        }

        fn get_config(&self, _key: &str) -> Option<serde_json::Value> {
            None
        }

        fn set_config(&mut self, _key: &str, _value: serde_json::Value) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_register_plugin() {
        let registry = PluginRegistry::new();
        let plugin = MockPlugin::new("test");

        registry.register(Box::new(plugin)).await.unwrap();

        assert!(registry.is_registered("test").await);
        assert_eq!(registry.count().await, 1);
    }

    #[tokio::test]
    async fn test_register_duplicate() {
        let registry = PluginRegistry::new();

        registry
            .register(Box::new(MockPlugin::new("test")))
            .await
            .unwrap();

        let result = registry.register(Box::new(MockPlugin::new("test"))).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_unregister_plugin() {
        let registry = PluginRegistry::new();

        registry
            .register(Box::new(MockPlugin::new("test")))
            .await
            .unwrap();
        assert!(registry.is_registered("test").await);

        registry.unregister("test").await.unwrap();
        assert!(!registry.is_registered("test").await);
    }

    #[tokio::test]
    async fn test_list_plugins() {
        let registry = PluginRegistry::new();

        registry
            .register(Box::new(MockPlugin::new("plugin-a")))
            .await
            .unwrap();
        registry
            .register(Box::new(MockPlugin::new("plugin-b")))
            .await
            .unwrap();

        let plugins = registry.list().await;
        assert_eq!(plugins.len(), 2);
    }

    #[tokio::test]
    async fn test_init_all() {
        let registry = PluginRegistry::new();

        registry
            .register(Box::new(MockPlugin::new("test")))
            .await
            .unwrap();

        let results = registry.init_all().await;
        assert_eq!(results.len(), 1);
        assert!(results[0].1.is_ok());
    }

    #[tokio::test]
    async fn test_add_remote_registry() {
        let registry = PluginRegistry::new();

        let remote = RemoteRegistry::new("https://example.com", "Example Registry");
        registry.add_remote_registry(remote).await;

        let registries = registry.list_remote_registries().await;
        assert_eq!(registries.len(), 1);
        assert_eq!(registries[0].name, "Example Registry");
        assert!(registries[0].enabled);
    }

    #[tokio::test]
    async fn test_add_duplicate_remote_registry() {
        let registry = PluginRegistry::new();

        let remote1 = RemoteRegistry::new("https://example.com", "Example Registry");
        let remote2 = RemoteRegistry::new("https://example.com", "Duplicate");

        registry.add_remote_registry(remote1).await;
        registry.add_remote_registry(remote2).await;

        let registries = registry.list_remote_registries().await;
        assert_eq!(registries.len(), 1);
    }

    #[tokio::test]
    async fn test_remove_remote_registry() {
        let registry = PluginRegistry::new();

        registry
            .add_remote_registry(RemoteRegistry::new("https://example.com", "Test"))
            .await;
        registry.remove_remote_registry("https://example.com").await;

        let registries = registry.list_remote_registries().await;
        assert!(registries.is_empty());
    }

    #[tokio::test]
    async fn test_search_empty_cache() {
        let registry = PluginRegistry::new();

        let results = registry.search("test").await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_plugin_index_entry_is_signed() {
        let entry = PluginIndexEntry {
            id: "test-plugin".to_string(),
            name: "Test Plugin".to_string(),
            version: "1.0.0".to_string(),
            description: "A test plugin".to_string(),
            download_url: "https://example.com/plugin.wasm".to_string(),
            checksum: "abc123".to_string(),
            signature: Some("signature".to_string()),
            updated_at: chrono::Utc::now(),
        };

        assert!(entry.is_signed());

        let unsigned_entry = PluginIndexEntry {
            signature: None,
            ..entry
        };

        assert!(!unsigned_entry.is_signed());
    }

    #[tokio::test]
    async fn test_remote_registry_disabled() {
        let registry = PluginRegistry::new();

        let remote = RemoteRegistry::new_disabled("https://example.com", "Disabled");
        registry.add_remote_registry(remote.clone()).await;

        // Fetching from disabled registry should return empty
        let result = registry.fetch_remote_index(&remote).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_clear_index_cache() {
        let registry = PluginRegistry::new();

        // Manually insert something into the cache
        {
            let mut cached = registry.cached_index.write().await;
            cached.insert("test".to_string(), vec![]);
        }

        registry.clear_index_cache().await;

        let cached = registry.cached_index.read().await;
        assert!(cached.is_empty());
    }

    // =========================================================================
    // Security Tests: SSRF Protection
    // =========================================================================

    #[test]
    fn test_ssrf_blocks_private_ipv4_addresses() {
        // Localhost
        assert!(PluginRegistry::validate_download_url("https://127.0.0.1/plugin.wasm").is_err());
        assert!(PluginRegistry::validate_download_url("https://127.0.0.255/plugin.wasm").is_err());

        // Private class A (10.0.0.0/8)
        assert!(PluginRegistry::validate_download_url("https://10.0.0.1/plugin.wasm").is_err());
        assert!(
            PluginRegistry::validate_download_url("https://10.255.255.255/plugin.wasm").is_err()
        );

        // Private class B (172.16.0.0/12)
        assert!(PluginRegistry::validate_download_url("https://172.16.0.1/plugin.wasm").is_err());
        assert!(
            PluginRegistry::validate_download_url("https://172.31.255.255/plugin.wasm").is_err()
        );

        // Private class C (192.168.0.0/16)
        assert!(PluginRegistry::validate_download_url("https://192.168.0.1/plugin.wasm").is_err());
        assert!(
            PluginRegistry::validate_download_url("https://192.168.255.255/plugin.wasm").is_err()
        );

        // Link-local / AWS metadata endpoint
        assert!(
            PluginRegistry::validate_download_url("https://169.254.169.254/plugin.wasm").is_err()
        );
        assert!(PluginRegistry::validate_download_url("https://169.254.0.1/plugin.wasm").is_err());

        // Carrier-grade NAT (100.64.0.0/10)
        assert!(PluginRegistry::validate_download_url("https://100.64.0.1/plugin.wasm").is_err());
        assert!(
            PluginRegistry::validate_download_url("https://100.127.255.255/plugin.wasm").is_err()
        );
    }

    #[test]
    fn test_ssrf_blocks_private_ipv6_addresses() {
        // Loopback
        assert!(PluginRegistry::validate_download_url("https://[::1]/plugin.wasm").is_err());

        // Link-local
        assert!(PluginRegistry::validate_download_url("https://[fe80::1]/plugin.wasm").is_err());

        // Unique local
        assert!(PluginRegistry::validate_download_url("https://[fc00::1]/plugin.wasm").is_err());
        assert!(PluginRegistry::validate_download_url("https://[fd00::1]/plugin.wasm").is_err());
    }

    #[test]
    fn test_ssrf_blocks_localhost_domains() {
        assert!(PluginRegistry::validate_download_url("https://localhost/plugin.wasm").is_err());
        assert!(PluginRegistry::validate_download_url("https://test.local/plugin.wasm").is_err());
        assert!(
            PluginRegistry::validate_download_url("https://internal.internal/plugin.wasm").is_err()
        );
    }

    #[test]
    fn test_ssrf_blocks_dangerous_ports() {
        // SSH
        assert!(
            PluginRegistry::validate_download_url("https://example.com:22/plugin.wasm").is_err()
        );
        // MySQL
        assert!(
            PluginRegistry::validate_download_url("https://example.com:3306/plugin.wasm").is_err()
        );
        // Redis
        assert!(
            PluginRegistry::validate_download_url("https://example.com:6379/plugin.wasm").is_err()
        );
        // MongoDB
        assert!(
            PluginRegistry::validate_download_url("https://example.com:27017/plugin.wasm").is_err()
        );
    }

    #[test]
    fn test_ssrf_blocks_non_https() {
        // File protocol
        assert!(PluginRegistry::validate_download_url("file:///etc/passwd").is_err());
        // FTP
        assert!(PluginRegistry::validate_download_url("ftp://example.com/plugin.wasm").is_err());
        // HTTP to non-localhost
        assert!(PluginRegistry::validate_download_url("http://example.com/plugin.wasm").is_err());
    }

    #[test]
    fn test_ssrf_allows_valid_https_urls() {
        assert!(PluginRegistry::validate_download_url("https://example.com/plugin.wasm").is_ok());
        assert!(
            PluginRegistry::validate_download_url(
                "https://plugins.cortex.dev/v1/download/test-plugin.wasm"
            )
            .is_ok()
        );
        assert!(
            PluginRegistry::validate_download_url(
                "https://github.com/user/repo/releases/download/v1.0.0/plugin.wasm"
            )
            .is_ok()
        );
    }

    #[test]
    fn test_ssrf_allows_standard_ports() {
        assert!(
            PluginRegistry::validate_download_url("https://example.com:443/plugin.wasm").is_ok()
        );
        assert!(
            PluginRegistry::validate_download_url("https://example.com:8000/plugin.wasm").is_ok()
        );
    }

    // =========================================================================
    // Security Tests: Directory Traversal Protection
    // =========================================================================

    #[tokio::test]
    async fn test_directory_traversal_blocks_dotdot() {
        let registry = PluginRegistry::new();
        let entry = PluginIndexEntry {
            id: "../../../etc/passwd".to_string(),
            name: "Malicious Plugin".to_string(),
            version: "1.0.0".to_string(),
            description: "Tries to escape".to_string(),
            download_url: "https://example.com/plugin.wasm".to_string(),
            checksum: "abc123".to_string(),
            signature: None,
            updated_at: chrono::Utc::now(),
        };

        let result = registry.download_plugin(&entry, Path::new("/tmp")).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("path") || err_msg.contains("invalid"));
    }

    #[tokio::test]
    async fn test_directory_traversal_blocks_forward_slash() {
        let registry = PluginRegistry::new();
        let entry = PluginIndexEntry {
            id: "plugin/subdir/file".to_string(),
            name: "Malicious Plugin".to_string(),
            version: "1.0.0".to_string(),
            description: "Tries to create subdirs".to_string(),
            download_url: "https://example.com/plugin.wasm".to_string(),
            checksum: "abc123".to_string(),
            signature: None,
            updated_at: chrono::Utc::now(),
        };

        let result = registry.download_plugin(&entry, Path::new("/tmp")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_directory_traversal_blocks_backslash() {
        let registry = PluginRegistry::new();
        let entry = PluginIndexEntry {
            id: "plugin\\..\\..\\etc".to_string(),
            name: "Malicious Plugin".to_string(),
            version: "1.0.0".to_string(),
            description: "Windows-style traversal".to_string(),
            download_url: "https://example.com/plugin.wasm".to_string(),
            checksum: "abc123".to_string(),
            signature: None,
            updated_at: chrono::Utc::now(),
        };

        let result = registry.download_plugin(&entry, Path::new("/tmp")).await;
        assert!(result.is_err());
    }
}
