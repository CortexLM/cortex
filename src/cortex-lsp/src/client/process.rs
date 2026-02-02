//! LSP server process management.

use crate::{Diagnostic, DiagnosticSeverity, LspError, Result};
use lsp_types::*;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};

use super::capabilities::CachedServerCapabilities;
use super::LspClient;

impl LspClient {
    /// Start the LSP server process.
    pub async fn start(&self) -> Result<()> {
        if self.config.command.is_empty() {
            return Err(LspError::StartFailed("No command specified".into()));
        }

        let mut cmd = Command::new(&self.config.command[0]);
        if self.config.command.len() > 1 {
            cmd.args(&self.config.command[1..]);
        }

        for (key, value) in &self.config.env {
            cmd.env(key, value);
        }

        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| {
            LspError::StartFailed(format!("Failed to spawn {}: {}", self.config.command[0], e))
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| LspError::StartFailed("Failed to get stdin".into()))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| LspError::StartFailed("Failed to get stdout".into()))?;

        *self.stdin.lock().await = Some(stdin);

        // Mark server as alive
        self.server_alive.store(true, Ordering::SeqCst);

        // Create shutdown channel for the response reader
        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
        *self.shutdown_tx.lock().await = Some(shutdown_tx);

        // Start reading responses in background
        let pending = self.pending_requests.clone();
        let diagnostics = self.diagnostics.clone();
        let server_alive = self.server_alive.clone();
        let read_timeout = self.client_config.read_timeout;
        let max_content_length = self.client_config.max_content_length;

        tokio::spawn(async move {
            Self::read_responses(
                stdout,
                pending,
                diagnostics,
                server_alive.clone(),
                shutdown_rx,
                read_timeout,
                max_content_length,
            )
            .await;
            // Mark server as dead when response reader exits
            server_alive.store(false, Ordering::SeqCst);
        });

        // Store the process
        *self.process.lock().await = Some(child);

        // Start process monitor to detect crashes (Unix only)
        #[cfg(unix)]
        {
            let server_alive_monitor = self.server_alive.clone();
            let server_name = self.config.name.clone();
            let pending_for_cleanup = self.pending_requests.clone();

            // Get the process handle for monitoring
            let process_guard = self.process.lock().await;
            if let Some(ref child) = *process_guard {
                let child_id = child.id();
                drop(process_guard); // Release the lock before spawning

                tokio::spawn(async move {
                    // Monitor process by periodically checking if it's still running
                    loop {
                        tokio::time::sleep(Duration::from_secs(5)).await;

                        if !server_alive_monitor.load(Ordering::SeqCst) {
                            break;
                        }

                        // Check if process exists using kill -0
                        use std::process::Command as StdCommand;
                        let exists = StdCommand::new("kill")
                            .args(["-0", &child_id.unwrap_or(0).to_string()])
                            .output()
                            .map(|o| o.status.success())
                            .unwrap_or(false);

                        if !exists && server_alive_monitor.load(Ordering::SeqCst) {
                            warn!("LSP server '{}' process has died unexpectedly", server_name);
                            server_alive_monitor.store(false, Ordering::SeqCst);

                            // Clean up pending requests
                            let mut pending = pending_for_cleanup.write().await;
                            pending.clear();
                            break;
                        }
                    }
                });
            } else {
                drop(process_guard);
            }
        }

        // On Windows, we rely on the response reader detecting EOF
        #[cfg(windows)]
        {
            // No additional process monitoring needed - the response reader task
            // will detect EOF when the process exits and set server_alive to false
        }

        info!("Started LSP server: {}", self.config.name);
        Ok(())
    }

    /// Initialize the LSP server.
    #[allow(deprecated)] // root_uri is deprecated but still widely used
    pub async fn initialize(&self) -> Result<()> {
        let params = InitializeParams {
            process_id: Some(std::process::id()),
            root_uri: self.root_uri.clone(),
            capabilities: ClientCapabilities {
                text_document: Some(TextDocumentClientCapabilities {
                    synchronization: Some(TextDocumentSyncClientCapabilities {
                        dynamic_registration: Some(false),
                        will_save: Some(false),
                        will_save_wait_until: Some(false),
                        did_save: Some(true),
                    }),
                    completion: Some(CompletionClientCapabilities {
                        dynamic_registration: Some(false),
                        completion_item: Some(CompletionItemCapability {
                            snippet_support: Some(true),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    hover: Some(HoverClientCapabilities {
                        dynamic_registration: Some(false),
                        content_format: Some(vec![MarkupKind::Markdown, MarkupKind::PlainText]),
                    }),
                    publish_diagnostics: Some(PublishDiagnosticsClientCapabilities {
                        related_information: Some(true),
                        ..Default::default()
                    }),
                    implementation: Some(GotoCapability {
                        dynamic_registration: Some(false),
                        link_support: Some(true),
                    }),
                    call_hierarchy: Some(CallHierarchyClientCapabilities {
                        dynamic_registration: Some(false),
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            initialization_options: if self.config.init_options.is_null() {
                None
            } else {
                Some(self.config.init_options.clone())
            },
            ..Default::default()
        };

        let result: InitializeResult = self.request("initialize", params).await?;

        // Cache server capabilities
        let cached_caps = CachedServerCapabilities::from_initialize_result(&result);
        *self.capabilities.write().await = Some(cached_caps);

        // Send initialized notification
        self.notify("initialized", InitializedParams {}).await?;

        *self.initialized.write().await = true;
        info!("LSP server initialized: {}", self.config.name);
        Ok(())
    }

    /// Shutdown the LSP server.
    pub async fn shutdown(&self) -> Result<()> {
        // Signal the response reader to stop
        if let Some(tx) = self.shutdown_tx.lock().await.take() {
            let _ = tx.send(()).await;
        }

        // Mark server as not alive
        self.server_alive.store(false, Ordering::SeqCst);

        // Try to send shutdown request if server is still responsive
        let shutdown_result = tokio::time::timeout(
            Duration::from_secs(5),
            self.request::<Value, Value>("shutdown", Value::Null),
        )
        .await;

        match shutdown_result {
            Ok(Ok(_)) => {
                // Server acknowledged shutdown, send exit notification
                let _ = self.notify("exit", Value::Null).await;
            }
            Ok(Err(e)) => {
                warn!("Shutdown request failed: {}", e);
            }
            Err(_) => {
                warn!("Shutdown request timed out");
            }
        }

        // Kill the process if it's still running
        if let Some(mut process) = self.process.lock().await.take() {
            let _ = process.kill().await;
        }

        // Clean up pending requests
        let mut pending = self.pending_requests.write().await;
        pending.clear();

        info!("LSP server shutdown: {}", self.config.name);
        Ok(())
    }

    /// Send a message to the LSP server.
    pub(super) async fn send_message(&self, message: &Value) -> Result<()> {
        let content = serde_json::to_string(message)?;
        let header = format!("Content-Length: {}\r\n\r\n", content.len());

        let mut stdin = self.stdin.lock().await;
        if let Some(stdin) = stdin.as_mut() {
            stdin.write_all(header.as_bytes()).await?;
            stdin.write_all(content.as_bytes()).await?;
            stdin.flush().await?;
            debug!(
                "Sent LSP message: {}",
                message.get("method").unwrap_or(&serde_json::Value::Null)
            );
            Ok(())
        } else {
            Err(LspError::Communication("Server not started".into()))
        }
    }

    /// Read responses from the LSP server stdout.
    pub(super) async fn read_responses(
        stdout: tokio::process::ChildStdout,
        pending: Arc<RwLock<HashMap<u64, mpsc::Sender<Value>>>>,
        diagnostics: Arc<RwLock<HashMap<PathBuf, Vec<Diagnostic>>>>,
        server_alive: Arc<std::sync::atomic::AtomicBool>,
        mut shutdown_rx: mpsc::Receiver<()>,
        read_timeout: Duration,
        max_content_length: usize,
    ) {
        let mut reader = BufReader::new(stdout);

        loop {
            // Check for shutdown signal
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    debug!("Response reader received shutdown signal");
                    break;
                }
                result = Self::read_single_message(&mut reader, read_timeout, max_content_length) => {
                    match result {
                        Ok(Some(message)) => {
                            // Handle response
                            if let Some(id) = message.get("id").and_then(|i| i.as_u64()) {
                                let tx = pending.write().await.remove(&id);
                                if let Some(tx) = tx {
                                    if tx.send(message).await.is_err() {
                                        debug!("Failed to send response for request {}: receiver dropped", id);
                                    }
                                }
                            }
                            // Handle notification
                            else if let Some(method) = message.get("method").and_then(|m| m.as_str()) {
                                if method == "textDocument/publishDiagnostics" {
                                    if let Some(params) = message.get("params") {
                                        Self::handle_diagnostics(params, &diagnostics).await;
                                    }
                                }
                            }
                        }
                        Ok(None) => {
                            // EOF - server closed stdout
                            debug!("LSP server closed stdout (EOF)");
                            break;
                        }
                        Err(e) => {
                            error!("Error reading LSP response: {}", e);
                            // Continue trying to read unless it's a fatal error
                            if e.contains("timeout") {
                                // Timeout reading - may indicate a malformed message or stuck server
                                warn!("Timeout reading LSP message, continuing...");
                                continue;
                            }
                            // For other errors, break the loop
                            break;
                        }
                    }
                }
            }
        }

        // Mark server as not alive when we exit
        server_alive.store(false, Ordering::SeqCst);

        // Clean up any remaining pending requests
        let mut pending_guard = pending.write().await;
        if !pending_guard.is_empty() {
            warn!(
                "Cleaning up {} pending requests due to response reader exit",
                pending_guard.len()
            );
            pending_guard.clear();
        }
    }

    /// Read a single LSP message from the reader with timeout.
    async fn read_single_message(
        reader: &mut BufReader<tokio::process::ChildStdout>,
        read_timeout: Duration,
        max_content_length: usize,
    ) -> std::result::Result<Option<Value>, String> {
        // Read headers with timeout
        let mut content_length: usize = 0;

        loop {
            let mut line = String::new();
            let read_result = tokio::time::timeout(read_timeout, reader.read_line(&mut line)).await;

            match read_result {
                Ok(Ok(0)) => {
                    // EOF
                    return Ok(None);
                }
                Ok(Ok(_)) => {
                    if line == "\r\n" || line == "\n" {
                        // End of headers
                        break;
                    }

                    if let Some(len_str) = line.strip_prefix("Content-Length: ") {
                        content_length = len_str
                            .trim()
                            .parse()
                            .map_err(|e| format!("Invalid Content-Length: {}", e))?;

                        // Validate content length to prevent memory exhaustion
                        if content_length > max_content_length {
                            return Err(format!(
                                "Content-Length {} exceeds maximum allowed {}",
                                content_length, max_content_length
                            ));
                        }
                    }
                }
                Ok(Err(e)) => {
                    return Err(format!("IO error reading headers: {}", e));
                }
                Err(_) => {
                    return Err("timeout reading headers".to_string());
                }
            }
        }

        if content_length == 0 {
            // Invalid message format, skip
            return Err("Invalid message: no Content-Length header".to_string());
        }

        // Read content with timeout
        let mut content = vec![0u8; content_length];
        let read_result = tokio::time::timeout(read_timeout, reader.read_exact(&mut content)).await;

        match read_result {
            Ok(Ok(_bytes_read)) => {
                // Parse JSON
                serde_json::from_slice(&content)
                    .map(Some)
                    .map_err(|e| format!("JSON parse error: {}", e))
            }
            Ok(Err(e)) => Err(format!("IO error reading content: {}", e)),
            Err(_) => Err("timeout reading content".to_string()),
        }
    }

    /// Handle diagnostics notification from the server.
    pub(super) async fn handle_diagnostics(
        params: &Value,
        diagnostics: &RwLock<HashMap<PathBuf, Vec<Diagnostic>>>,
    ) {
        let uri = params.get("uri").and_then(|u| u.as_str());
        let diags = params.get("diagnostics").and_then(|d| d.as_array());

        if let (Some(uri), Some(diags)) = (uri, diags) {
            if let Ok(url) = Url::parse(uri) {
                if let Ok(path) = url.to_file_path() {
                    let converted: Vec<Diagnostic> = diags
                        .iter()
                        .filter_map(|d| {
                            let range = d.get("range")?;
                            let start = range.get("start")?;
                            let line = start.get("line")?.as_u64()? as u32 + 1;
                            let column = start.get("character")?.as_u64()? as u32 + 1;
                            let message = d.get("message")?.as_str()?.to_string();
                            let severity = d
                                .get("severity")
                                .and_then(|s| s.as_u64())
                                .map(|s| match s {
                                    1 => DiagnosticSeverity::Error,
                                    2 => DiagnosticSeverity::Warning,
                                    3 => DiagnosticSeverity::Information,
                                    _ => DiagnosticSeverity::Hint,
                                })
                                .unwrap_or(DiagnosticSeverity::Error);

                            let mut diag = Diagnostic::new(path.clone(), line, column, message)
                                .with_severity(severity);

                            if let Some(source) = d.get("source").and_then(|s| s.as_str()) {
                                diag = diag.with_source(source);
                            }

                            if let Some(code) = d.get("code") {
                                let code_str = if code.is_string() {
                                    code.as_str().map(String::from)
                                } else if code.is_number() {
                                    Some(code.to_string())
                                } else {
                                    None
                                };
                                if let Some(c) = code_str {
                                    diag = diag.with_code(c);
                                }
                            }

                            Some(diag)
                        })
                        .collect();

                    diagnostics.write().await.insert(path, converted);
                }
            }
        }
    }
}
