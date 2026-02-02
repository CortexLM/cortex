//! LSP client implementation.

mod capabilities;
mod config;
mod process;
mod requests;

pub use capabilities::CachedServerCapabilities;
pub use config::LspClientConfig;

use crate::{Diagnostic, LspError, LspServerConfig, Result};
use lsp_types::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::process::Child;
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::warn;

/// LSP client for communicating with a language server.
pub struct LspClient {
    pub(crate) config: LspServerConfig,
    pub(crate) client_config: LspClientConfig,
    pub(crate) process: Mutex<Option<Child>>,
    pub(crate) stdin: Mutex<Option<tokio::process::ChildStdin>>,
    pub(crate) request_id: AtomicU64,
    pub(crate) pending_requests: Arc<RwLock<HashMap<u64, mpsc::Sender<Value>>>>,
    pub(crate) root_uri: Option<Url>,
    pub(crate) initialized: RwLock<bool>,
    pub(crate) diagnostics: Arc<RwLock<HashMap<PathBuf, Vec<Diagnostic>>>>,
    /// Flag indicating if the server process has crashed or stopped.
    pub(crate) server_alive: Arc<AtomicBool>,
    /// Cached server capabilities.
    pub(crate) capabilities: RwLock<Option<CachedServerCapabilities>>,
    /// Shutdown signal sender for the response reader task.
    pub(crate) shutdown_tx: Mutex<Option<mpsc::Sender<()>>>,
}

impl LspClient {
    /// Create a new LSP client with default configuration.
    pub fn new(config: LspServerConfig) -> Self {
        Self::with_client_config(config, LspClientConfig::default())
    }

    /// Create a new LSP client with custom client configuration.
    pub fn with_client_config(config: LspServerConfig, client_config: LspClientConfig) -> Self {
        Self {
            config,
            client_config,
            process: Mutex::new(None),
            stdin: Mutex::new(None),
            request_id: AtomicU64::new(1),
            pending_requests: Arc::new(RwLock::new(HashMap::new())),
            root_uri: None,
            initialized: RwLock::new(false),
            diagnostics: Arc::new(RwLock::new(HashMap::new())),
            server_alive: Arc::new(AtomicBool::new(false)),
            capabilities: RwLock::new(None),
            shutdown_tx: Mutex::new(None),
        }
    }

    /// Set the workspace root for this client.
    pub fn with_root(mut self, root: &Path) -> Self {
        self.root_uri = Url::from_file_path(root).ok();
        self
    }

    /// Check if the server process is still alive.
    pub fn is_server_alive(&self) -> bool {
        self.server_alive.load(Ordering::SeqCst)
    }

    /// Get the cached server capabilities.
    pub async fn get_capabilities(&self) -> Option<CachedServerCapabilities> {
        self.capabilities.read().await.clone()
    }

    /// Check if the server supports a specific capability.
    pub(crate) async fn check_capability(&self, capability_name: &str) -> Result<()> {
        let caps = self.capabilities.read().await;
        if let Some(ref caps) = *caps {
            let supported = match capability_name {
                "hover" => caps.hover,
                "definition" => caps.goto_definition,
                "references" => caps.find_references,
                "documentSymbol" => caps.document_symbol,
                "workspaceSymbol" => caps.workspace_symbol,
                "completion" => caps.completion,
                "signatureHelp" => caps.signature_help,
                "rename" => caps.rename,
                "formatting" => caps.formatting,
                "implementation" => caps.implementation,
                "callHierarchy" => caps.call_hierarchy,
                "codeAction" => caps.code_action,
                _ => true, // Unknown capabilities are assumed supported
            };
            if !supported {
                return Err(LspError::Communication(format!(
                    "Server does not support '{}' capability",
                    capability_name
                )));
            }
        }
        Ok(())
    }

    /// Ensure the server is alive before making requests.
    fn ensure_server_alive(&self) -> Result<()> {
        if !self.server_alive.load(Ordering::SeqCst) {
            return Err(LspError::Communication(
                "LSP server has crashed or is not running".into(),
            ));
        }
        Ok(())
    }

    /// Send a request to the LSP server.
    pub async fn request<P: Serialize, R: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        params: P,
    ) -> Result<R> {
        // Ensure server is alive before making request
        self.ensure_server_alive()?;

        let id = self.request_id.fetch_add(1, Ordering::SeqCst);

        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        let (tx, mut rx) = mpsc::channel(1);
        self.pending_requests.write().await.insert(id, tx);

        // Send message with error handling
        if let Err(e) = self.send_message(&request).await {
            // Clean up the pending request on send failure
            self.pending_requests.write().await.remove(&id);
            return Err(e);
        }

        // Wait for response with configurable timeout
        let timeout_result =
            tokio::time::timeout(self.client_config.request_timeout, rx.recv()).await;

        // Clean up the pending request regardless of outcome
        self.pending_requests.write().await.remove(&id);

        let response = match timeout_result {
            Ok(Some(response)) => response,
            Ok(None) => {
                // Channel was closed, server likely crashed
                return Err(LspError::Communication(
                    "Response channel closed unexpectedly".into(),
                ));
            }
            Err(_) => {
                // Timeout occurred
                warn!(
                    "LSP request '{}' (id={}) timed out after {:?}",
                    method, id, self.client_config.request_timeout
                );
                return Err(LspError::Timeout);
            }
        };

        if let Some(error) = response.get("error") {
            return Err(LspError::Communication(error.to_string()));
        }

        let result = response
            .get("result")
            .ok_or_else(|| LspError::Communication("No result in response".into()))?;

        serde_json::from_value(result.clone()).map_err(|e| e.into())
    }

    /// Send a notification to the LSP server.
    pub async fn notify<P: Serialize>(&self, method: &str, params: P) -> Result<()> {
        let notification = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });

        self.send_message(&notification).await
    }

    /// Get diagnostics for a file.
    pub async fn get_diagnostics(&self, path: &Path) -> Vec<Diagnostic> {
        self.diagnostics
            .read()
            .await
            .get(path)
            .cloned()
            .unwrap_or_default()
    }

    /// Get all diagnostics.
    pub async fn all_diagnostics(&self) -> HashMap<PathBuf, Vec<Diagnostic>> {
        self.diagnostics.read().await.clone()
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        // Process will be killed when dropped
    }
}
