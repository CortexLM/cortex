//! Tool registry - manages tool definitions, handlers, and plugins.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use super::spec::{ToolDefinition, ToolHandler, ToolResult};
use crate::error::Result;

mod definitions;
mod executors;
mod plugins;
mod types;

pub use types::PluginTool;

/// Registry of available tools.
#[derive(Default)]
pub struct ToolRegistry {
    pub(crate) tools: HashMap<String, ToolDefinition>,
    pub(crate) plugins: HashMap<String, PluginTool>,
    pub(crate) handlers: HashMap<String, Arc<dyn ToolHandler>>,
    /// LSP integration.
    pub(crate) lsp: Option<Arc<crate::integrations::LspIntegration>>,
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("tools", &self.tools)
            .field("plugins", &self.plugins)
            .field("handlers_count", &self.handlers.len())
            .finish()
    }
}

impl ToolRegistry {
    /// Create a new registry with default tools.
    pub fn new() -> Self {
        let mut registry = Self::default();
        registry.register_default_tools();
        registry
    }

    /// Create a new registry and load plugins from standard directories.
    pub async fn new_with_plugins() -> Self {
        let mut registry = Self::new();

        // Load plugins from ~/.cortex/plugins
        if let Some(home) = dirs::home_dir() {
            let plugins_dir = home.join(".cortex").join("plugins");
            if let Ok(count) = registry.load_plugins_from_dir(&plugins_dir).await
                && count > 0
            {
                tracing::info!("Loaded {} plugins from {}", count, plugins_dir.display());
            }
        }

        // Load plugins from .cortex/plugins in current directory
        let local_plugins = std::path::Path::new(".cortex/plugins");
        if let Ok(count) = registry.load_plugins_from_dir(local_plugins).await
            && count > 0
        {
            tracing::info!("Loaded {} local plugins", count);
        }

        registry
    }

    /// Register a tool.
    pub fn register(&mut self, tool: ToolDefinition) {
        self.tools.insert(tool.name.clone(), tool);
    }

    /// Register a tool with handler.
    pub fn register_with_handler(&mut self, tool: ToolDefinition, handler: Arc<dyn ToolHandler>) {
        self.tools.insert(tool.name.clone(), tool);
        self.handlers.insert(handler.name().to_string(), handler);
    }

    /// Set the LSP integration.
    pub fn set_lsp(&mut self, lsp: Arc<crate::integrations::LspIntegration>) {
        self.lsp = Some(lsp);
    }

    /// Get a tool definition.
    pub fn get(&self, name: &str) -> Option<&ToolDefinition> {
        self.tools.get(name)
    }

    /// Get all tool definitions.
    pub fn all(&self) -> Vec<&ToolDefinition> {
        self.tools.values().collect()
    }

    /// Get tool definitions for API.
    pub fn get_definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().cloned().collect()
    }

    /// Check if a tool is registered.
    pub fn has(&self, name: &str) -> bool {
        self.tools.contains_key(name)
            || self.plugins.contains_key(name)
            || self.handlers.contains_key(name)
    }

    /// Execute a tool.
    pub async fn execute(&self, name: &str, arguments: Value) -> Result<ToolResult> {
        let mut context =
            super::context::ToolContext::new(std::env::current_dir().unwrap_or_default());
        if let Some(lsp) = &self.lsp {
            context = context.with_lsp(lsp.clone());
        }
        self.execute_with_context(name, arguments, context).await
    }

    /// Execute a tool with a custom context (for output streaming support).
    pub async fn execute_with_context(
        &self,
        name: &str,
        arguments: Value,
        context: super::context::ToolContext,
    ) -> Result<ToolResult> {
        if !self.has(name) {
            return Ok(ToolResult::error(format!("Unknown tool: {name}")));
        }

        // Check if it's a dynamic handler first
        if let Some(handler) = self.handlers.get(name) {
            return handler.execute(arguments, &context).await;
        }

        // Check if it's a plugin tool next
        if let Some(plugin) = self.plugins.get(name) {
            return self.execute_plugin(plugin, arguments).await;
        }

        // Dispatch to handler based on tool name
        // Note: "Execute" is handled by LocalShellHandler via the handlers map above
        match name {
            "Read" => self.execute_read_file(arguments).await,
            "Create" => self.execute_write_file(arguments).await,
            "LS" => self.execute_list_dir(arguments).await,
            "SearchFiles" => self.execute_search_files(arguments).await,
            "Edit" => self.execute_edit_file(arguments).await,
            "Grep" => self.execute_grep(arguments).await,
            "Glob" => self.execute_glob(arguments).await,
            "FetchUrl" | "WebFetch" => self.execute_fetch_url(arguments).await,
            "TodoWrite" => self.execute_todo_write(arguments).await,
            "TodoRead" => self.execute_todo_read(arguments).await,
            "Task" => self.execute_task(arguments).await,
            "ListSubagents" => self.execute_list_subagents(arguments).await,
            "LspDiagnostics" => self.execute_lsp_diagnostics(arguments).await,
            "LspHover" => self.execute_lsp_hover(arguments).await,
            "LspSymbols" => self.execute_lsp_symbols(arguments).await,
            _ => Ok(ToolResult::error(format!("Tool not implemented: {name}"))),
        }
    }
}
