//! Type definitions for the tool registry.

use std::path::PathBuf;

use serde_json::Value;

/// A custom plugin tool definition.
#[derive(Debug, Clone)]
pub struct PluginTool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub script_path: PathBuf,
}
