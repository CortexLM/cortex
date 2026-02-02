//! Plugin API for WASM plugins to interact with Cortex.
//!
//! This module provides the host functions that plugins can call
//! to interact with the Cortex system.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::{PluginError, Result};

/// Context provided to plugins during execution.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginContext {
    /// Current session ID
    pub session_id: Option<String>,

    /// Current message ID
    pub message_id: Option<String>,

    /// Working directory
    pub cwd: PathBuf,

    /// Agent name
    pub agent: Option<String>,

    /// Current model
    pub model: Option<String>,

    /// Plugin ID (set by the system)
    pub plugin_id: Option<String>,

    /// Extra data
    #[serde(default)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl PluginContext {
    /// Create a new context with the working directory.
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            ..Default::default()
        }
    }

    /// Set the session ID.
    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Set the message ID.
    pub fn with_message(mut self, message_id: impl Into<String>) -> Self {
        self.message_id = Some(message_id.into());
        self
    }

    /// Set the agent name.
    pub fn with_agent(mut self, agent: impl Into<String>) -> Self {
        self.agent = Some(agent.into());
        self
    }

    /// Set the model name.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set the plugin ID.
    pub fn with_plugin(mut self, plugin_id: impl Into<String>) -> Self {
        self.plugin_id = Some(plugin_id.into());
        self
    }

    /// Add extra data.
    pub fn with_extra(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.extra.insert(key.into(), value);
        self
    }
}

/// Log level for plugin logging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// Command execution result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResult {
    /// Exit code
    pub exit_code: i32,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
    /// Whether the command succeeded
    pub success: bool,
}

/// HTTP request options.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpRequest {
    /// URL to request
    pub url: String,
    /// HTTP method
    pub method: String,
    /// Request headers
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Request body
    #[serde(default)]
    pub body: Option<String>,
    /// Timeout in milliseconds
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_timeout() -> u64 {
    30000
}

/// HTTP response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpResponse {
    /// Status code
    pub status: u16,
    /// Response headers
    pub headers: HashMap<String, String>,
    /// Response body
    pub body: String,
}

/// Plugin API trait - the interface plugins use to interact with Cortex.
#[async_trait::async_trait]
pub trait PluginApi: Send + Sync {
    // ========== File System ==========

    /// Read a file.
    async fn read_file(&self, path: &str) -> Result<String>;

    /// Write a file.
    async fn write_file(&self, path: &str, content: &str) -> Result<()>;

    /// Check if a file exists.
    async fn file_exists(&self, path: &str) -> Result<bool>;

    /// List directory contents.
    async fn list_dir(&self, path: &str) -> Result<Vec<String>>;

    /// Create a directory.
    async fn create_dir(&self, path: &str) -> Result<()>;

    /// Delete a file.
    async fn delete_file(&self, path: &str) -> Result<()>;

    // ========== Shell ==========

    /// Execute a shell command.
    async fn execute(&self, command: &[String]) -> Result<CommandResult>;

    // ========== Network ==========

    /// Make an HTTP request.
    async fn http_request(&self, request: HttpRequest) -> Result<HttpResponse>;

    // ========== Logging ==========

    /// Log a message.
    fn log(&self, level: LogLevel, message: &str);

    // ========== Configuration ==========

    /// Get a configuration value.
    async fn get_config(&self, key: &str) -> Option<serde_json::Value>;

    /// Set a configuration value.
    async fn set_config(&self, key: &str, value: serde_json::Value) -> Result<()>;

    // ========== Storage ==========

    /// Get a stored value for the plugin.
    async fn storage_get(&self, key: &str) -> Option<serde_json::Value>;

    /// Set a stored value for the plugin.
    async fn storage_set(&self, key: &str, value: serde_json::Value) -> Result<()>;

    /// Delete a stored value.
    async fn storage_delete(&self, key: &str) -> Result<()>;

    // ========== UI ==========

    /// Show a notification.
    async fn show_notification(&self, title: &str, message: &str) -> Result<()>;

    /// Copy text to clipboard.
    async fn copy_to_clipboard(&self, text: &str) -> Result<()>;
}

/// Host functions provided to WASM plugins.
///
/// This struct implements the actual functionality that WASM plugins
/// can call through the API.
pub struct PluginHostFunctions {
    /// Working directory
    cwd: PathBuf,

    /// Plugin ID
    plugin_id: String,

    /// Allowed file paths
    allowed_paths: Vec<PathBuf>,

    /// Allowed commands
    allowed_commands: Vec<String>,

    /// Allowed network domains
    allowed_domains: Option<Vec<String>>,

    /// Plugin storage
    storage: Arc<RwLock<HashMap<String, serde_json::Value>>>,

    /// Configuration
    config: Arc<RwLock<HashMap<String, serde_json::Value>>>,
}

impl PluginHostFunctions {
    /// Create new host functions.
    pub fn new(plugin_id: &str, cwd: PathBuf) -> Self {
        Self {
            cwd,
            plugin_id: plugin_id.to_string(),
            allowed_paths: vec![],
            allowed_commands: vec![],
            allowed_domains: None,
            storage: Arc::new(RwLock::new(HashMap::new())),
            config: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Allow access to specific paths.
    pub fn with_allowed_paths(mut self, paths: Vec<PathBuf>) -> Self {
        self.allowed_paths = paths;
        self
    }

    /// Allow specific commands.
    pub fn with_allowed_commands(mut self, commands: Vec<String>) -> Self {
        self.allowed_commands = commands;
        self
    }

    /// Allow access to specific domains.
    pub fn with_allowed_domains(mut self, domains: Vec<String>) -> Self {
        self.allowed_domains = Some(domains);
        self
    }

    /// Set initial configuration.
    pub async fn with_config(self, config: HashMap<String, serde_json::Value>) -> Self {
        *self.config.write().await = config;
        self
    }

    /// Resolve a path relative to the working directory.
    ///
    /// # Security
    ///
    /// This method canonicalizes the path to prevent path traversal attacks
    /// using sequences like `../../../etc/passwd`. The canonical path is
    /// verified to be within the allowed directory boundaries.
    fn resolve_path(&self, path: &str) -> Result<PathBuf> {
        let path = std::path::Path::new(path);
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.cwd.join(path)
        };

        // SECURITY: Canonicalize to resolve `..`, `.`, and symlinks
        // This prevents path traversal attacks
        let canonical = resolved.canonicalize().map_err(|e| {
            PluginError::PermissionDenied(format!("Invalid path '{}': {}", path.display(), e))
        })?;

        // SECURITY: Verify the canonical path is within allowed boundaries
        // If no explicit allowlist, only allow paths within cwd
        if self.allowed_paths.is_empty() {
            // Canonicalize cwd for comparison
            let canonical_cwd = self.cwd.canonicalize().map_err(|e| {
                PluginError::PermissionDenied(format!("Invalid working directory: {}", e))
            })?;

            if !canonical.starts_with(&canonical_cwd) {
                return Err(PluginError::PermissionDenied(format!(
                    "Path '{}' escapes working directory",
                    path.display()
                )));
            }
        }

        Ok(canonical)
    }

    /// Check if a path is allowed.
    ///
    /// # Security
    ///
    /// - Empty allowlist = only paths within cwd are allowed (fail-closed)
    /// - Paths are canonicalized before checking to prevent traversal attacks
    /// - Symlinks are resolved to their real paths
    fn is_path_allowed(&self, path: &Path) -> bool {
        // SECURITY: Try to canonicalize the path to resolve symlinks and ..
        let canonical = match path.canonicalize() {
            Ok(p) => p,
            // SECURITY: If we can't canonicalize, deny access (fail-closed)
            Err(_) => return false,
        };

        // SECURITY: If allowlist is empty, only allow paths within cwd
        if self.allowed_paths.is_empty() {
            // Canonicalize cwd for comparison
            return match self.cwd.canonicalize() {
                Ok(canonical_cwd) => canonical.starts_with(&canonical_cwd),
                // SECURITY: If cwd can't be canonicalized, deny access
                Err(_) => false,
            };
        }

        // Check against canonicalized allowlist entries
        for allowed in &self.allowed_paths {
            if let Ok(allowed_canonical) = allowed.canonicalize() {
                if canonical.starts_with(&allowed_canonical) {
                    return true;
                }
            }
        }

        false
    }

    /// Check if a command is allowed.
    ///
    /// # Security
    ///
    /// Empty allowlist = no commands allowed (fail-closed).
    /// Use "*" in allowlist to permit all commands (requires explicit opt-in).
    fn is_command_allowed(&self, command: &str) -> bool {
        // SECURITY: Fail-closed - empty allowlist means no commands allowed
        if self.allowed_commands.is_empty() {
            return false;
        }

        self.allowed_commands
            .iter()
            .any(|c| c == command || c == "*")
    }

    /// Check if a domain is allowed for network requests.
    ///
    /// # Security
    ///
    /// - None = no network access allowed (fail-closed)
    /// - Empty list = no network access allowed (fail-closed)
    /// - Only http/https protocols are allowed
    /// - Localhost and private IPs are blocked to prevent SSRF
    /// - Dangerous ports are blocked
    fn is_domain_allowed(&self, url: &str) -> bool {
        // SECURITY: Fail-closed - None means no network access
        let Some(ref domains) = self.allowed_domains else {
            return false;
        };

        // SECURITY: Fail-closed - empty list means no network access
        if domains.is_empty() {
            return false;
        }

        let Ok(parsed) = url::Url::parse(url) else {
            return false;
        };

        // SECURITY: Only allow http/https protocols to prevent file://, ftp://, etc.
        match parsed.scheme() {
            "http" | "https" => {}
            _ => {
                tracing::warn!(
                    scheme = parsed.scheme(),
                    url = url,
                    "Blocked non-HTTP protocol in plugin request"
                );
                return false;
            }
        }

        let Some(host) = parsed.host_str() else {
            return false;
        };

        // SECURITY: Block localhost and private IP addresses to prevent SSRF
        if Self::is_private_host(host) {
            tracing::warn!(
                host = host,
                url = url,
                "Blocked private/localhost address in plugin request (SSRF prevention)"
            );
            return false;
        }

        // SECURITY: Block dangerous ports commonly used by internal services
        if let Some(port) = parsed.port() {
            if Self::is_dangerous_port(port) {
                tracing::warn!(
                    port = port,
                    url = url,
                    "Blocked dangerous port in plugin request"
                );
                return false;
            }
        }

        domains
            .iter()
            .any(|d| host == d || host.ends_with(&format!(".{}", d)))
    }

    /// Check if a host is a private/localhost address.
    ///
    /// # Security
    ///
    /// This prevents SSRF attacks by blocking access to:
    /// - localhost and loopback addresses
    /// - Private IP ranges (10.x, 172.16-31.x, 192.168.x)
    /// - Link-local addresses
    /// - .local and .internal domains
    fn is_private_host(host: &str) -> bool {
        // Localhost variations
        if host == "localhost"
            || host == "127.0.0.1"
            || host == "::1"
            || host == "0.0.0.0"
            || host == "[::1]"
        {
            return true;
        }

        // Private IPv4 ranges
        if host.starts_with("192.168.") || host.starts_with("10.") || host.starts_with("169.254.")
        // Link-local
        {
            return true;
        }

        // Private 172.16.0.0 - 172.31.255.255 range
        if host.starts_with("172.") {
            if let Some(second_octet) = host.split('.').nth(1) {
                if let Ok(octet) = second_octet.parse::<u8>() {
                    if (16..=31).contains(&octet) {
                        return true;
                    }
                }
            }
        }

        // Private domain suffixes
        if host.ends_with(".local")
            || host.ends_with(".internal")
            || host.ends_with(".localhost")
            || host.ends_with(".localdomain")
        {
            return true;
        }

        // IPv6 private/link-local (simplified check)
        if host.starts_with("fe80:") // Link-local
            || host.starts_with("fc00:") // Unique local
            || host.starts_with("fd")
        // Unique local
        {
            return true;
        }

        false
    }

    /// Check if a port is commonly used by internal/dangerous services.
    ///
    /// # Security
    ///
    /// Blocks ports commonly used by:
    /// - SSH, SMTP, and other system services
    /// - Database servers (MySQL, PostgreSQL, MongoDB, Redis, etc.)
    /// - Cloud metadata services (169.254.169.254:80)
    fn is_dangerous_port(port: u16) -> bool {
        const BLOCKED_PORTS: &[u16] = &[
            22,    // SSH
            23,    // Telnet
            25,    // SMTP
            53,    // DNS
            110,   // POP3
            135,   // RPC
            139,   // NetBIOS
            143,   // IMAP
            445,   // SMB
            1433,  // MSSQL
            1521,  // Oracle
            3306,  // MySQL
            3389,  // RDP
            5432,  // PostgreSQL
            5900,  // VNC
            6379,  // Redis
            6380,  // Redis (alt)
            9200,  // Elasticsearch
            9300,  // Elasticsearch (transport)
            11211, // Memcached
            27017, // MongoDB
            27018, // MongoDB (alt)
            28017, // MongoDB (web)
        ];

        BLOCKED_PORTS.contains(&port)
    }
}

#[async_trait::async_trait]
impl PluginApi for PluginHostFunctions {
    async fn read_file(&self, path: &str) -> Result<String> {
        // SECURITY: resolve_path now canonicalizes and validates the path
        let full_path = self.resolve_path(path)?;

        if !self.is_path_allowed(&full_path) {
            return Err(PluginError::PermissionDenied(format!(
                "Access to path '{}' is not allowed",
                path
            )));
        }

        tokio::fs::read_to_string(&full_path)
            .await
            .map_err(|e| PluginError::execution_error(&self.plugin_id, e.to_string()))
    }

    async fn write_file(&self, path: &str, content: &str) -> Result<()> {
        // SECURITY: For write operations, we need to handle non-existent files
        // First, validate the parent directory exists and is allowed
        let path_obj = std::path::Path::new(path);
        let resolved = if path_obj.is_absolute() {
            path_obj.to_path_buf()
        } else {
            self.cwd.join(path_obj)
        };

        // Get the parent directory and canonicalize it
        let parent = resolved.parent().ok_or_else(|| {
            PluginError::PermissionDenied(format!("Invalid path '{}': no parent directory", path))
        })?;

        // SECURITY: Canonicalize parent to prevent traversal attacks
        let canonical_parent = parent.canonicalize().map_err(|e| {
            PluginError::PermissionDenied(format!("Invalid parent directory for '{}': {}", path, e))
        })?;

        // The full path would be the canonical parent plus the filename
        let filename = resolved.file_name().ok_or_else(|| {
            PluginError::PermissionDenied(format!("Invalid path '{}': no filename", path))
        })?;
        let full_path = canonical_parent.join(filename);

        if !self.is_path_allowed(&canonical_parent) {
            return Err(PluginError::PermissionDenied(format!(
                "Access to path '{}' is not allowed",
                path
            )));
        }

        tokio::fs::write(&full_path, content)
            .await
            .map_err(|e| PluginError::execution_error(&self.plugin_id, e.to_string()))
    }

    async fn file_exists(&self, path: &str) -> Result<bool> {
        // SECURITY: For existence checks, we need to be careful about timing attacks
        // and information disclosure. We'll try to resolve, but return false on error.
        let full_path = match self.resolve_path(path) {
            Ok(p) => p,
            // If we can't resolve (path doesn't exist or is invalid), return false
            Err(_) => return Ok(false),
        };

        if !self.is_path_allowed(&full_path) {
            return Err(PluginError::PermissionDenied(format!(
                "Access to path '{}' is not allowed",
                path
            )));
        }

        Ok(tokio::fs::try_exists(&full_path).await.unwrap_or(false))
    }

    async fn list_dir(&self, path: &str) -> Result<Vec<String>> {
        // SECURITY: resolve_path now canonicalizes and validates the path
        let full_path = self.resolve_path(path)?;

        if !self.is_path_allowed(&full_path) {
            return Err(PluginError::PermissionDenied(format!(
                "Access to path '{}' is not allowed",
                path
            )));
        }

        let mut entries = Vec::new();
        let mut dir = tokio::fs::read_dir(&full_path)
            .await
            .map_err(|e| PluginError::execution_error(&self.plugin_id, e.to_string()))?;

        while let Some(entry) = dir
            .next_entry()
            .await
            .map_err(|e| PluginError::execution_error(&self.plugin_id, e.to_string()))?
        {
            if let Some(name) = entry.file_name().to_str() {
                entries.push(name.to_string());
            }
        }

        Ok(entries)
    }

    async fn create_dir(&self, path: &str) -> Result<()> {
        // SECURITY: For directory creation, validate parent path is allowed
        let path_obj = std::path::Path::new(path);
        let resolved = if path_obj.is_absolute() {
            path_obj.to_path_buf()
        } else {
            self.cwd.join(path_obj)
        };

        // Find the first existing parent and canonicalize it
        let mut check_path = resolved.clone();
        let mut existing_parent = None;
        while let Some(parent) = check_path.parent() {
            if parent.exists() {
                existing_parent = Some(parent.to_path_buf());
                break;
            }
            check_path = parent.to_path_buf();
        }

        let canonical_parent = existing_parent
            .ok_or_else(|| {
                PluginError::PermissionDenied(format!("No valid parent directory for '{}'", path))
            })?
            .canonicalize()
            .map_err(|e| {
                PluginError::PermissionDenied(format!(
                    "Invalid parent directory for '{}': {}",
                    path, e
                ))
            })?;

        if !self.is_path_allowed(&canonical_parent) {
            return Err(PluginError::PermissionDenied(format!(
                "Access to path '{}' is not allowed",
                path
            )));
        }

        tokio::fs::create_dir_all(&resolved)
            .await
            .map_err(|e| PluginError::execution_error(&self.plugin_id, e.to_string()))
    }

    async fn delete_file(&self, path: &str) -> Result<()> {
        // SECURITY: resolve_path now canonicalizes and validates the path
        let full_path = self.resolve_path(path)?;

        if !self.is_path_allowed(&full_path) {
            return Err(PluginError::PermissionDenied(format!(
                "Access to path '{}' is not allowed",
                path
            )));
        }

        tokio::fs::remove_file(&full_path)
            .await
            .map_err(|e| PluginError::execution_error(&self.plugin_id, e.to_string()))
    }

    async fn execute(&self, command: &[String]) -> Result<CommandResult> {
        if command.is_empty() {
            return Err(PluginError::execution_error(
                &self.plugin_id,
                "Empty command",
            ));
        }

        if !self.is_command_allowed(&command[0]) {
            return Err(PluginError::PermissionDenied(format!(
                "Command '{}' is not allowed",
                command[0]
            )));
        }

        let output = tokio::process::Command::new(&command[0])
            .args(&command[1..])
            .current_dir(&self.cwd)
            .output()
            .await
            .map_err(|e| PluginError::execution_error(&self.plugin_id, e.to_string()))?;

        Ok(CommandResult {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            success: output.status.success(),
        })
    }

    async fn http_request(&self, request: HttpRequest) -> Result<HttpResponse> {
        if !self.is_domain_allowed(&request.url) {
            return Err(PluginError::PermissionDenied(format!(
                "Access to URL '{}' is not allowed",
                request.url
            )));
        }

        let client = reqwest::Client::new();

        let method = request.method.to_uppercase();
        let mut req = match method.as_str() {
            "GET" => client.get(&request.url),
            "POST" => client.post(&request.url),
            "PUT" => client.put(&request.url),
            "DELETE" => client.delete(&request.url),
            "PATCH" => client.patch(&request.url),
            "HEAD" => client.head(&request.url),
            _ => {
                return Err(PluginError::execution_error(
                    &self.plugin_id,
                    format!("Unsupported HTTP method: {}", method),
                ));
            }
        };

        for (key, value) in &request.headers {
            req = req.header(key, value);
        }

        if let Some(body) = request.body {
            req = req.body(body);
        }

        let response = req
            .timeout(std::time::Duration::from_millis(request.timeout_ms))
            .send()
            .await
            .map_err(|e| PluginError::execution_error(&self.plugin_id, e.to_string()))?;

        let status = response.status().as_u16();
        let headers: HashMap<String, String> = response
            .headers()
            .iter()
            .filter_map(|(k, v)| Some((k.to_string(), v.to_str().ok()?.to_string())))
            .collect();

        let body = response
            .text()
            .await
            .map_err(|e| PluginError::execution_error(&self.plugin_id, e.to_string()))?;

        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }

    fn log(&self, level: LogLevel, message: &str) {
        match level {
            LogLevel::Trace => tracing::trace!(plugin = %self.plugin_id, "{}", message),
            LogLevel::Debug => tracing::debug!(plugin = %self.plugin_id, "{}", message),
            LogLevel::Info => tracing::info!(plugin = %self.plugin_id, "{}", message),
            LogLevel::Warn => tracing::warn!(plugin = %self.plugin_id, "{}", message),
            LogLevel::Error => tracing::error!(plugin = %self.plugin_id, "{}", message),
        }
    }

    async fn get_config(&self, key: &str) -> Option<serde_json::Value> {
        self.config.read().await.get(key).cloned()
    }

    async fn set_config(&self, key: &str, value: serde_json::Value) -> Result<()> {
        self.config.write().await.insert(key.to_string(), value);
        Ok(())
    }

    async fn storage_get(&self, key: &str) -> Option<serde_json::Value> {
        self.storage.read().await.get(key).cloned()
    }

    async fn storage_set(&self, key: &str, value: serde_json::Value) -> Result<()> {
        self.storage.write().await.insert(key.to_string(), value);
        Ok(())
    }

    async fn storage_delete(&self, key: &str) -> Result<()> {
        self.storage.write().await.remove(key);
        Ok(())
    }

    async fn show_notification(&self, title: &str, message: &str) -> Result<()> {
        // This would integrate with the TUI notification system
        tracing::info!(
            plugin = %self.plugin_id,
            title = %title,
            "Plugin notification: {}",
            message
        );
        Ok(())
    }

    async fn copy_to_clipboard(&self, text: &str) -> Result<()> {
        // This would integrate with the clipboard system
        tracing::debug!(
            plugin = %self.plugin_id,
            "Copy to clipboard: {} chars",
            text.len()
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_context() {
        let ctx = PluginContext::new("/tmp")
            .with_session("session-123")
            .with_agent("build")
            .with_model("gpt-4");

        assert_eq!(ctx.session_id, Some("session-123".to_string()));
        assert_eq!(ctx.agent, Some("build".to_string()));
        assert_eq!(ctx.model, Some("gpt-4".to_string()));
    }

    #[test]
    fn test_host_functions_path_allowed_with_explicit_allowlist() {
        // Use a temp directory that works cross-platform
        let temp_dir = std::env::temp_dir();
        let host = PluginHostFunctions::new("test", temp_dir.clone())
            .with_allowed_paths(vec![temp_dir.clone()]);

        // Temp dir should be allowed when explicitly added to allowlist
        assert!(host.is_path_allowed(&temp_dir));
    }

    #[test]
    fn test_host_functions_command_allowed() {
        let host = PluginHostFunctions::new("test", PathBuf::from("/tmp"))
            .with_allowed_commands(vec!["ls".to_string(), "cat".to_string()]);

        assert!(host.is_command_allowed("ls"));
        assert!(host.is_command_allowed("cat"));
        assert!(!host.is_command_allowed("rm"));
    }

    #[test]
    fn test_host_functions_command_empty_allowlist_fails_closed() {
        // SECURITY: Empty command allowlist should deny all commands (fail-closed)
        let host = PluginHostFunctions::new("test", PathBuf::from("/tmp"));

        assert!(!host.is_command_allowed("ls"));
        assert!(!host.is_command_allowed("cat"));
        assert!(!host.is_command_allowed("rm"));
    }

    #[test]
    fn test_host_functions_command_wildcard() {
        let host = PluginHostFunctions::new("test", PathBuf::from("/tmp"))
            .with_allowed_commands(vec!["*".to_string()]);

        assert!(host.is_command_allowed("ls"));
        assert!(host.is_command_allowed("cat"));
        assert!(host.is_command_allowed("rm"));
    }

    #[test]
    fn test_host_functions_domain_allowed() {
        let host = PluginHostFunctions::new("test", PathBuf::from("/tmp"))
            .with_allowed_domains(vec!["example.com".to_string()]);

        assert!(host.is_domain_allowed("https://example.com/api"));
        assert!(host.is_domain_allowed("https://api.example.com/v1"));
        assert!(!host.is_domain_allowed("https://other.com/api"));
    }

    #[test]
    fn test_host_functions_domain_empty_allowlist_fails_closed() {
        // SECURITY: Empty domain allowlist should deny all requests (fail-closed)
        let host =
            PluginHostFunctions::new("test", PathBuf::from("/tmp")).with_allowed_domains(vec![]);

        assert!(!host.is_domain_allowed("https://example.com/api"));
        assert!(!host.is_domain_allowed("https://api.example.com/v1"));
    }

    #[test]
    fn test_host_functions_domain_none_fails_closed() {
        // SECURITY: None allowed_domains should deny all requests (fail-closed)
        let host = PluginHostFunctions::new("test", PathBuf::from("/tmp"));

        assert!(!host.is_domain_allowed("https://example.com/api"));
    }

    #[test]
    fn test_host_functions_ssrf_prevention() {
        let host = PluginHostFunctions::new("test", PathBuf::from("/tmp"))
            .with_allowed_domains(vec!["*".to_string(), "example.com".to_string()]);

        // SECURITY: These should all be blocked for SSRF prevention
        assert!(!host.is_domain_allowed("http://localhost/api"));
        assert!(!host.is_domain_allowed("http://127.0.0.1/api"));
        assert!(!host.is_domain_allowed("http://192.168.1.1/api"));
        assert!(!host.is_domain_allowed("http://10.0.0.1/api"));
        assert!(!host.is_domain_allowed("http://172.16.0.1/api"));
        assert!(!host.is_domain_allowed("http://169.254.169.254/latest/meta-data/")); // AWS metadata
        assert!(!host.is_domain_allowed("http://internal.local/api"));
    }

    #[test]
    fn test_host_functions_protocol_restriction() {
        let host = PluginHostFunctions::new("test", PathBuf::from("/tmp"))
            .with_allowed_domains(vec!["example.com".to_string()]);

        // SECURITY: Only http/https should be allowed
        assert!(host.is_domain_allowed("https://example.com/api"));
        assert!(host.is_domain_allowed("http://example.com/api"));
        assert!(!host.is_domain_allowed("file:///etc/passwd"));
        assert!(!host.is_domain_allowed("ftp://example.com/file"));
        assert!(!host.is_domain_allowed("gopher://example.com/"));
    }

    #[test]
    fn test_host_functions_dangerous_ports_blocked() {
        let host = PluginHostFunctions::new("test", PathBuf::from("/tmp"))
            .with_allowed_domains(vec!["example.com".to_string()]);

        // SECURITY: Dangerous ports should be blocked
        assert!(!host.is_domain_allowed("https://example.com:22/api")); // SSH
        assert!(!host.is_domain_allowed("https://example.com:3306/api")); // MySQL
        assert!(!host.is_domain_allowed("https://example.com:5432/api")); // PostgreSQL
        assert!(!host.is_domain_allowed("https://example.com:6379/api")); // Redis
        assert!(!host.is_domain_allowed("https://example.com:27017/api")); // MongoDB

        // Standard ports should be allowed
        assert!(host.is_domain_allowed("https://example.com:443/api"));
        assert!(host.is_domain_allowed("http://example.com:80/api"));
        assert!(host.is_domain_allowed("https://example.com:8080/api"));
    }

    #[test]
    fn test_is_private_host() {
        // Localhost variations
        assert!(PluginHostFunctions::is_private_host("localhost"));
        assert!(PluginHostFunctions::is_private_host("127.0.0.1"));
        assert!(PluginHostFunctions::is_private_host("::1"));
        assert!(PluginHostFunctions::is_private_host("0.0.0.0"));

        // Private IP ranges
        assert!(PluginHostFunctions::is_private_host("192.168.1.1"));
        assert!(PluginHostFunctions::is_private_host("10.0.0.1"));
        assert!(PluginHostFunctions::is_private_host("172.16.0.1"));
        assert!(PluginHostFunctions::is_private_host("172.31.255.255"));
        assert!(PluginHostFunctions::is_private_host("169.254.169.254")); // AWS metadata

        // Private domain suffixes
        assert!(PluginHostFunctions::is_private_host("server.local"));
        assert!(PluginHostFunctions::is_private_host("internal.internal"));
        assert!(PluginHostFunctions::is_private_host("app.localhost"));

        // Public should not be private
        assert!(!PluginHostFunctions::is_private_host("example.com"));
        assert!(!PluginHostFunctions::is_private_host("8.8.8.8"));
        assert!(!PluginHostFunctions::is_private_host("google.com"));
    }

    #[test]
    fn test_is_dangerous_port() {
        // Database ports
        assert!(PluginHostFunctions::is_dangerous_port(3306)); // MySQL
        assert!(PluginHostFunctions::is_dangerous_port(5432)); // PostgreSQL
        assert!(PluginHostFunctions::is_dangerous_port(27017)); // MongoDB
        assert!(PluginHostFunctions::is_dangerous_port(6379)); // Redis

        // System service ports
        assert!(PluginHostFunctions::is_dangerous_port(22)); // SSH
        assert!(PluginHostFunctions::is_dangerous_port(25)); // SMTP
        assert!(PluginHostFunctions::is_dangerous_port(445)); // SMB

        // Safe ports
        assert!(!PluginHostFunctions::is_dangerous_port(80));
        assert!(!PluginHostFunctions::is_dangerous_port(443));
        assert!(!PluginHostFunctions::is_dangerous_port(8080));
        assert!(!PluginHostFunctions::is_dangerous_port(8443));
    }
}
