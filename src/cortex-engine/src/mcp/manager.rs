//! MCP Connection Manager - Manages multiple MCP server connections.
//!
//! Provides a unified interface to:
//! - Connect/disconnect multiple MCP servers
//! - List all available tools across servers
//! - Execute tools on the appropriate server
//! - Handle server lifecycle (auto-start, restart)

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Result, anyhow};
use serde_json::Value;
use tokio::sync::{Mutex, RwLock, mpsc};
use tracing::warn;

use cortex_mcp_types::{CallToolResult, Resource, Tool};

use super::McpServerConfig;
use super::client::{ConnectionState, McpClient};

/// MCP lifecycle event for notifications
#[derive(Debug, Clone)]
pub enum McpLifecycleEvent {
    /// Server was added to the manager
    ServerAdded { name: String },
    /// Server was connected and tools discovered
    ServerConnected {
        name: String,
        tool_count: usize,
        tool_names: Vec<String>,
    },
    /// Server was disconnected
    ServerDisconnected { name: String },
    /// Server was removed from the manager
    ServerRemoved { name: String },
    /// Connection failed
    ConnectionFailed { name: String, error: String },
}

/// Delimiter used to create qualified tool names.
const TOOL_NAME_DELIMITER: &str = "__";

/// Prefix for MCP tools.
const MCP_TOOL_PREFIX: &str = "mcp";

/// MCP Connection Manager - manages multiple server connections.
pub struct McpConnectionManager {
    /// Connected clients by server name.
    clients: RwLock<HashMap<String, Arc<McpClient>>>,
    /// Server configurations.
    configs: RwLock<HashMap<String, McpServerConfig>>,
    /// Set of servers currently being started (prevents race conditions).
    starting_servers: Mutex<HashSet<String>>,
    /// Event sender for lifecycle notifications
    event_tx: Option<mpsc::UnboundedSender<McpLifecycleEvent>>,
}

impl Default for McpConnectionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl McpConnectionManager {
    /// Create a new connection manager.
    pub fn new() -> Self {
        Self {
            clients: RwLock::new(HashMap::new()),
            configs: RwLock::new(HashMap::new()),
            starting_servers: Mutex::new(HashSet::new()),
            event_tx: None,
        }
    }

    /// Create a new connection manager with an event sender
    pub fn with_event_sender(tx: mpsc::UnboundedSender<McpLifecycleEvent>) -> Self {
        Self {
            clients: RwLock::new(HashMap::new()),
            configs: RwLock::new(HashMap::new()),
            starting_servers: Mutex::new(HashSet::new()),
            event_tx: Some(tx),
        }
    }

    /// Set the event sender for lifecycle notifications
    pub fn set_event_sender(&mut self, tx: mpsc::UnboundedSender<McpLifecycleEvent>) {
        self.event_tx = Some(tx);
    }

    /// Helper to send an event
    fn send_event(&self, event: McpLifecycleEvent) {
        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(event);
        }
    }

    /// Add a server configuration.
    pub async fn add_server(&self, config: McpServerConfig) {
        let name = config.name.clone();
        self.configs
            .write()
            .await
            .insert(name.clone(), config.clone());

        // Create client but don't connect yet
        let client = Arc::new(McpClient::new(config));
        self.clients.write().await.insert(name.clone(), client);

        self.send_event(McpLifecycleEvent::ServerAdded { name });
    }

    /// Remove a server.
    pub async fn remove_server(&self, name: &str) -> Result<()> {
        self.send_event(McpLifecycleEvent::ServerRemoved {
            name: name.to_string(),
        });

        // Disconnect if connected
        if let Some(client) = self.clients.write().await.remove(name) {
            client.disconnect().await?;
        }
        self.configs.write().await.remove(name);
        Ok(())
    }

    /// Get a client by name.
    pub async fn get_client(&self, name: &str) -> Option<Arc<McpClient>> {
        self.clients.read().await.get(name).cloned()
    }

    /// Get all server names.
    pub async fn server_names(&self) -> Vec<String> {
        self.clients.read().await.keys().cloned().collect()
    }

    /// Connect to a specific server.
    /// Uses a lock to prevent concurrent startup of the same server (race condition fix).
    pub async fn connect(&self, name: &str) -> Result<()> {
        // Check if this server is already being started by another request
        {
            let mut starting = self.starting_servers.lock().await;
            if starting.contains(name) {
                // Server is already being started, wait and check again
                drop(starting);
                // Wait a bit for the other request to complete
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                // Check if connection succeeded
                let client = self
                    .clients
                    .read()
                    .await
                    .get(name)
                    .cloned()
                    .ok_or_else(|| anyhow!("Server not found: {}", name))?;

                if client.is_connected().await {
                    return Ok(());
                }

                // Still not connected, wait for the other startup to complete
                for _ in 0..50 {
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    if client.is_connected().await {
                        return Ok(());
                    }
                    // Check if startup failed (no longer in starting set)
                    let starting = self.starting_servers.lock().await;
                    if !starting.contains(name) {
                        break;
                    }
                }

                // Other startup may have failed - fall through to try our own connection
                // instead of recursing (which would require boxing the future)
            } else {
                // Mark this server as starting
                starting.insert(name.to_string());
            }
        }

        // Ensure we remove from starting set when done
        let result = async {
            let client = self
                .clients
                .read()
                .await
                .get(name)
                .cloned()
                .ok_or_else(|| anyhow!("Server not found: {}", name))?;

            client.connect().await
        }
        .await;

        // Remove from starting set
        {
            let mut starting = self.starting_servers.lock().await;
            starting.remove(name);
        }

        // Emit lifecycle event based on connection result
        match &result {
            Ok(()) => {
                // After successful connect, get tools and emit event
                if let Some(client) = self.clients.read().await.get(name).cloned() {
                    let tools = client.tools().await;
                    let tool_names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
                    self.send_event(McpLifecycleEvent::ServerConnected {
                        name: name.to_string(),
                        tool_count: tools.len(),
                        tool_names,
                    });
                }
            }
            Err(e) => {
                self.send_event(McpLifecycleEvent::ConnectionFailed {
                    name: name.to_string(),
                    error: e.to_string(),
                });
            }
        }

        result
    }

    /// Disconnect from a specific server.
    pub async fn disconnect(&self, name: &str) -> Result<()> {
        let client = self
            .clients
            .read()
            .await
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow!("Server not found: {}", name))?;

        let result = client.disconnect().await;

        if result.is_ok() {
            self.send_event(McpLifecycleEvent::ServerDisconnected {
                name: name.to_string(),
            });
        }

        result
    }

    /// Connect to all servers.
    pub async fn connect_all(&self) -> Vec<(String, Result<()>)> {
        let clients: Vec<_> = self
            .clients
            .read()
            .await
            .iter()
            .map(|(name, client)| (name.clone(), client.clone()))
            .collect();

        let mut results = Vec::new();
        for (name, client) in clients {
            let result = client.connect().await;
            results.push((name, result));
        }
        results
    }

    /// Disconnect from all servers.
    pub async fn disconnect_all(&self) -> Result<()> {
        let clients: Vec<_> = self.clients.read().await.values().cloned().collect();

        for client in clients {
            if let Err(e) = client.disconnect().await {
                warn!("Error disconnecting from {}: {}", client.name(), e);
            }
        }
        Ok(())
    }

    /// Get connection status for all servers.
    pub async fn status(&self) -> HashMap<String, ConnectionState> {
        let clients = self.clients.read().await;
        let mut status = HashMap::new();

        for (name, client) in clients.iter() {
            status.insert(name.clone(), client.state().await);
        }

        status
    }

    /// List all tools from all connected servers.
    /// Returns tools with qualified names: mcp__<server>__<tool>
    pub async fn list_all_tools(&self) -> HashMap<String, Tool> {
        let clients = self.clients.read().await;
        let mut all_tools = HashMap::new();

        for (server_name, client) in clients.iter() {
            if client.state().await != ConnectionState::Connected {
                continue;
            }

            for tool in client.tools().await {
                let qualified_name = format!(
                    "{}{}{}{}{}",
                    MCP_TOOL_PREFIX,
                    TOOL_NAME_DELIMITER,
                    server_name,
                    TOOL_NAME_DELIMITER,
                    tool.name
                );
                all_tools.insert(qualified_name, tool);
            }
        }

        all_tools
    }

    /// List all resources from all connected servers.
    pub async fn list_all_resources(&self) -> HashMap<String, Resource> {
        let clients = self.clients.read().await;
        let mut all_resources = HashMap::new();

        for (server_name, client) in clients.iter() {
            if client.state().await != ConnectionState::Connected {
                continue;
            }

            for resource in client.resources().await {
                let qualified_uri = format!("{}://{}", server_name, resource.uri);
                all_resources.insert(qualified_uri, resource);
            }
        }

        all_resources
    }

    /// Call a tool by its qualified name.
    pub async fn call_tool(
        &self,
        qualified_name: &str,
        arguments: Option<Value>,
    ) -> Result<CallToolResult> {
        let (server_name, tool_name) = parse_qualified_name(qualified_name)
            .ok_or_else(|| anyhow!("Invalid qualified tool name: {}", qualified_name))?;

        let client = self
            .clients
            .read()
            .await
            .get(&server_name)
            .cloned()
            .ok_or_else(|| anyhow!("MCP server not found: {}", server_name))?;

        if client.state().await != ConnectionState::Connected {
            return Err(anyhow!("MCP server not connected: {}", server_name));
        }

        client.call_tool(&tool_name, arguments).await
    }

    /// Read a resource by its qualified URI.
    pub async fn read_resource(
        &self,
        qualified_uri: &str,
    ) -> Result<cortex_mcp_types::ReadResourceResult> {
        // Parse server://uri format
        let parts: Vec<&str> = qualified_uri.splitn(2, "://").collect();
        if parts.len() != 2 {
            return Err(anyhow!("Invalid qualified resource URI: {}", qualified_uri));
        }

        let server_name = parts[0];
        let uri = parts[1];

        let client = self
            .clients
            .read()
            .await
            .get(server_name)
            .cloned()
            .ok_or_else(|| anyhow!("MCP server not found: {}", server_name))?;

        if client.state().await != ConnectionState::Connected {
            return Err(anyhow!("MCP server not connected: {}", server_name));
        }

        client.read_resource(uri).await
    }

    /// Refresh tools for all connected servers.
    pub async fn refresh_all_tools(&self) -> Result<()> {
        let clients: Vec<_> = self.clients.read().await.values().cloned().collect();

        for client in clients {
            if client.state().await == ConnectionState::Connected {
                if let Err(e) = client.refresh_tools().await {
                    warn!("Failed to refresh tools for {}: {}", client.name(), e);
                }
            }
        }
        Ok(())
    }

    /// Load servers from configuration.
    pub async fn load_from_configs(&self, configs: Vec<McpServerConfig>) {
        for config in configs {
            self.add_server(config).await;
        }
    }
}

/// Parse a qualified tool name into (server_name, tool_name).
pub fn parse_qualified_name(qualified_name: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = qualified_name.split(TOOL_NAME_DELIMITER).collect();

    if parts.len() < 3 || parts[0] != MCP_TOOL_PREFIX {
        return None;
    }

    let server_name = parts[1].to_string();
    let tool_name = parts[2..].join(TOOL_NAME_DELIMITER);

    if tool_name.is_empty() {
        return None;
    }

    Some((server_name, tool_name))
}

/// Create a qualified tool name from server and tool names.
pub fn create_qualified_name(server_name: &str, tool_name: &str) -> String {
    format!(
        "{}{}{}{}{}",
        MCP_TOOL_PREFIX, TOOL_NAME_DELIMITER, server_name, TOOL_NAME_DELIMITER, tool_name
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_qualified_name() {
        assert_eq!(
            parse_qualified_name("mcp__github__list_repos"),
            Some(("github".to_string(), "list_repos".to_string()))
        );

        assert_eq!(
            parse_qualified_name("mcp__server__tool__with__delimiters"),
            Some(("server".to_string(), "tool__with__delimiters".to_string()))
        );

        assert_eq!(parse_qualified_name("invalid"), None);
        assert_eq!(parse_qualified_name("mcp__server"), None);
        assert_eq!(parse_qualified_name("other__server__tool"), None);
    }

    #[test]
    fn test_create_qualified_name() {
        assert_eq!(
            create_qualified_name("github", "list_repos"),
            "mcp__github__list_repos"
        );
    }

    #[tokio::test]
    async fn test_manager_creation() {
        let manager = McpConnectionManager::new();
        assert!(manager.server_names().await.is_empty());
    }

    #[tokio::test]
    async fn test_add_remove_server() {
        let manager = McpConnectionManager::new();

        let config = McpServerConfig::new("test", "echo");
        manager.add_server(config).await;

        assert_eq!(manager.server_names().await, vec!["test".to_string()]);

        manager.remove_server("test").await.unwrap();
        assert!(manager.server_names().await.is_empty());
    }

    #[tokio::test]
    async fn test_lifecycle_events() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let manager = McpConnectionManager::with_event_sender(tx);

        let config = McpServerConfig::new("test", "echo");
        manager.add_server(config).await;

        // Should receive ServerAdded event
        let event = rx.try_recv();
        assert!(matches!(event, Ok(McpLifecycleEvent::ServerAdded { name }) if name == "test"));

        manager.remove_server("test").await.unwrap();

        // Should receive ServerRemoved event
        let event = rx.try_recv();
        assert!(matches!(event, Ok(McpLifecycleEvent::ServerRemoved { name }) if name == "test"));
    }

    #[tokio::test]
    async fn test_manager_with_event_sender() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let manager = McpConnectionManager::with_event_sender(tx);
        assert!(manager.event_tx.is_some());
    }

    #[tokio::test]
    async fn test_manager_set_event_sender() {
        let mut manager = McpConnectionManager::new();
        assert!(manager.event_tx.is_none());

        let (tx, _rx) = mpsc::unbounded_channel();
        manager.set_event_sender(tx);
        assert!(manager.event_tx.is_some());
    }
}
