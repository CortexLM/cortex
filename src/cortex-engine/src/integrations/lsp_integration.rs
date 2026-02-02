//! LSP integration for cortex-core.
//!
//! Connects the cortex-lsp crate to provide diagnostics in tool results.

use cortex_lsp::{Diagnostic, DiagnosticSeverity, LspManager};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::debug;

// Type aliases for LSP results (using Value for flexibility)
pub type Location = Value;
pub type DocumentSymbol = Value;
pub type WorkspaceSymbol = Value;
pub type CallHierarchyItem = Value;
pub type CallHierarchyIncomingCall = Value;
pub type CallHierarchyOutgoingCall = Value;
pub type CompletionItem = Value;
pub type SignatureHelp = Value;
pub type WorkspaceEdit = Value;
pub type CodeAction = Value;

/// LSP integration for the agent.
pub struct LspIntegration {
    manager: Arc<RwLock<Option<LspManager>>>,
    enabled: bool,
}

impl LspIntegration {
    /// Create a new LSP integration.
    pub fn new(enabled: bool) -> Self {
        Self {
            manager: Arc::new(RwLock::new(None)),
            enabled,
        }
    }

    /// Initialize LSP for a workspace.
    pub async fn init(&self, workspace_root: &Path) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let manager = LspManager::new(workspace_root.to_path_buf());
        debug!("Initialized LSP manager for {:?}", workspace_root);

        *self.manager.write().await = Some(manager);
        Ok(())
    }

    /// Get diagnostics for a file.
    pub async fn get_diagnostics(&self, file_path: &Path) -> Vec<Diagnostic> {
        let guard = self.manager.read().await;
        if let Some(ref manager) = *guard {
            manager.get_diagnostics(file_path).await
        } else {
            Vec::new()
        }
    }

    /// Get all diagnostics for the workspace.
    pub async fn get_all_diagnostics(&self) -> HashMap<PathBuf, Vec<Diagnostic>> {
        let guard = self.manager.read().await;
        if let Some(ref manager) = *guard {
            manager.all_diagnostics().await
        } else {
            HashMap::new()
        }
    }

    /// Get diagnostics for multiple files.
    pub async fn get_diagnostics_batch(
        &self,
        files: &[PathBuf],
    ) -> Vec<(PathBuf, Vec<Diagnostic>)> {
        let mut results = Vec::new();

        for file in files {
            let diagnostics = self.get_diagnostics(file).await;
            if !diagnostics.is_empty() {
                results.push((file.clone(), diagnostics));
            }
        }

        results
    }

    /// Get hover information for a position.
    pub async fn hover(
        &self,
        path: &str,
        line: u32,
        column: u32,
    ) -> anyhow::Result<Option<String>> {
        let guard = self.manager.read().await;
        if let Some(ref manager) = *guard {
            manager
                .hover(Path::new(path), line, column)
                .await
                .map_err(anyhow::Error::from)
        } else {
            Ok(None)
        }
    }

    /// Go to definition (stub - not yet implemented in cortex-lsp).
    pub async fn go_to_definition(
        &self,
        _path: &str,
        _line: u32,
        _column: u32,
    ) -> anyhow::Result<Vec<Location>> {
        // LSP feature not yet implemented - returns empty result
        // This will be implemented in a future version of cortex-lsp
        Ok(Vec::new())
    }

    /// Find references (stub - not yet implemented in cortex-lsp).
    pub async fn find_references(
        &self,
        _path: &str,
        _line: u32,
        _column: u32,
    ) -> anyhow::Result<Vec<Location>> {
        // LSP feature not yet implemented - returns empty result
        // This will be implemented in a future version of cortex-lsp
        Ok(Vec::new())
    }

    /// Get document symbols (stub - not yet implemented in cortex-lsp).
    pub async fn document_symbols(&self, _path: &str) -> anyhow::Result<Vec<DocumentSymbol>> {
        // LSP feature not yet implemented - returns empty result
        // This will be implemented in a future version of cortex-lsp
        Ok(Vec::new())
    }

    /// Search workspace symbols (stub - not yet implemented in cortex-lsp).
    pub async fn workspace_symbols(&self, _query: &str) -> anyhow::Result<Vec<WorkspaceSymbol>> {
        // LSP feature not yet implemented - returns empty result
        // This will be implemented in a future version of cortex-lsp
        Ok(Vec::new())
    }

    /// Go to implementation (stub - not yet implemented in cortex-lsp).
    pub async fn go_to_implementation(
        &self,
        _path: &str,
        _line: u32,
        _column: u32,
    ) -> anyhow::Result<Vec<Location>> {
        // LSP feature not yet implemented - returns empty result
        // This will be implemented in a future version of cortex-lsp
        Ok(Vec::new())
    }

    /// Prepare call hierarchy (stub - not yet implemented in cortex-lsp).
    pub async fn prepare_call_hierarchy(
        &self,
        _path: &str,
        _line: u32,
        _column: u32,
    ) -> anyhow::Result<Vec<CallHierarchyItem>> {
        // LSP feature not yet implemented - returns empty result
        // This will be implemented in a future version of cortex-lsp
        Ok(Vec::new())
    }

    /// Get incoming calls (stub - not yet implemented in cortex-lsp).
    pub async fn incoming_calls(
        &self,
        _path: &str,
        _line: u32,
        _column: u32,
    ) -> anyhow::Result<Vec<CallHierarchyIncomingCall>> {
        // LSP feature not yet implemented - returns empty result
        // This will be implemented in a future version of cortex-lsp
        Ok(Vec::new())
    }

    /// Get outgoing calls (stub - not yet implemented in cortex-lsp).
    pub async fn outgoing_calls(
        &self,
        _path: &str,
        _line: u32,
        _column: u32,
    ) -> anyhow::Result<Vec<CallHierarchyOutgoingCall>> {
        // LSP feature not yet implemented - returns empty result
        // This will be implemented in a future version of cortex-lsp
        Ok(Vec::new())
    }

    /// Get completions (stub - not yet implemented in cortex-lsp).
    pub async fn completions(
        &self,
        _path: &str,
        _line: u32,
        _column: u32,
    ) -> anyhow::Result<Vec<CompletionItem>> {
        // LSP feature not yet implemented - returns empty result
        // This will be implemented in a future version of cortex-lsp
        Ok(Vec::new())
    }

    /// Get signature help (stub - not yet implemented in cortex-lsp).
    pub async fn signature_help(
        &self,
        _path: &str,
        _line: u32,
        _column: u32,
    ) -> anyhow::Result<Option<SignatureHelp>> {
        // LSP feature not yet implemented - returns empty result
        // This will be implemented in a future version of cortex-lsp
        Ok(None)
    }

    /// Rename symbol (stub - not yet implemented in cortex-lsp).
    pub async fn rename(
        &self,
        _path: &str,
        _line: u32,
        _column: u32,
        _new_name: &str,
    ) -> anyhow::Result<WorkspaceEdit> {
        // LSP feature not yet implemented - returns empty result
        // This will be implemented in a future version of cortex-lsp
        Ok(Value::Null)
    }

    /// Get code actions (stub - not yet implemented in cortex-lsp).
    pub async fn code_actions(
        &self,
        _path: &str,
        _line: u32,
        _column: u32,
    ) -> anyhow::Result<Vec<CodeAction>> {
        // LSP feature not yet implemented - returns empty result
        // This will be implemented in a future version of cortex-lsp
        Ok(Vec::new())
    }

    /// Format diagnostics for display in tool results.
    pub fn format_diagnostics(diagnostics: &[Diagnostic]) -> String {
        if diagnostics.is_empty() {
            return String::new();
        }

        let mut output = String::from("\n\n--- LSP Diagnostics ---\n");

        let errors: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.severity == DiagnosticSeverity::Error)
            .collect();

        let warnings: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.severity == DiagnosticSeverity::Warning)
            .collect();

        if !errors.is_empty() {
            output.push_str(&format!("\nErrors ({}):\n", errors.len()));
            for diag in errors.iter().take(5) {
                output.push_str(&format!("  Line {}: {}\n", diag.line + 1, diag.message));
            }
            if errors.len() > 5 {
                output.push_str(&format!("  ... and {} more errors\n", errors.len() - 5));
            }
        }

        if !warnings.is_empty() {
            output.push_str(&format!("\nWarnings ({}):\n", warnings.len()));
            for diag in warnings.iter().take(3) {
                output.push_str(&format!("  Line {}: {}\n", diag.line + 1, diag.message));
            }
            if warnings.len() > 3 {
                output.push_str(&format!("  ... and {} more warnings\n", warnings.len() - 3));
            }
        }

        output
    }

    /// Shutdown LSP servers.
    pub async fn shutdown(&self) {
        *self.manager.write().await = None;
    }

    /// Check if LSP is enabled and running.
    pub async fn is_running(&self) -> bool {
        self.enabled && self.manager.read().await.is_some()
    }
}

impl Default for LspIntegration {
    fn default() -> Self {
        Self::new(false)
    }
}
