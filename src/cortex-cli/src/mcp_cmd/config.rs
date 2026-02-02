//! Configuration loading utilities for MCP commands.
//!
//! This module handles reading and parsing the MCP server configuration
//! from the cortex config file.

use anyhow::{Context, Result};
use cortex_engine::config::find_cortex_home;

/// Load the config file as a toml::Value.
pub(crate) fn load_config() -> Result<Option<toml::Value>> {
    let config_path = find_cortex_home()
        .map_err(|e| anyhow::anyhow!("Failed to find cortex home: {}", e))?
        .join("config.toml");

    if !config_path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read config: {}", config_path.display()))?;

    let config: toml::Value = toml::from_str(&content).with_context(|| "failed to parse config")?;

    Ok(Some(config))
}

/// Get all MCP servers from config.
pub(crate) fn get_mcp_servers() -> Result<toml::map::Map<String, toml::Value>> {
    let config = load_config()?;

    let servers = config
        .and_then(|c| c.get("mcp_servers").and_then(|v| v.as_table()).cloned())
        .unwrap_or_default();

    Ok(servers)
}

/// Get a specific MCP server from config.
pub(crate) fn get_mcp_server(name: &str) -> Result<Option<toml::Value>> {
    let servers = get_mcp_servers()?;
    Ok(servers.get(name).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_config_does_not_panic() {
        // load_config should not panic whether or not a config file exists
        let result = load_config();
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_mcp_servers_returns_ok() {
        // get_mcp_servers should return Ok regardless of config state
        let result = get_mcp_servers();
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_mcp_server_nonexistent() {
        // Requesting a non-existent server should return Ok(None)
        let result = get_mcp_server("nonexistent_server_xyz_12345");
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_get_mcp_servers_returns_map() {
        // The result should be a map (even if empty)
        let result = get_mcp_servers();
        assert!(result.is_ok());
        let servers = result.unwrap();
        // servers is a Map<String, Value>, we can iterate over it
        for (key, _value) in servers.iter() {
            // Each key should be a valid string (non-empty if present)
            assert!(!key.is_empty());
        }
    }

    #[test]
    fn test_get_mcp_server_with_empty_name() {
        // Even an empty name should not cause a panic, just return None
        let result = get_mcp_server("");
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }
}
