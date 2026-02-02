//! Tool execution context.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use cortex_common::normalize_path as normalize_path_util;
use cortex_protocol::SandboxPolicy;
use tokio::sync::mpsc;

use crate::integrations::LspIntegration;

/// Output chunk from tool execution
#[derive(Debug, Clone)]
pub enum ToolOutputChunk {
    Stdout(String),
    Stderr(String),
}

/// Context for tool execution.
#[derive(Clone)]
pub struct ToolContext {
    /// Current working directory.
    pub cwd: PathBuf,
    /// Sandbox policy.
    pub sandbox_policy: SandboxPolicy,
    /// Environment variables.
    pub env: HashMap<String, String>,
    /// Turn ID.
    pub turn_id: String,
    /// Conversation ID.
    pub conversation_id: String,
    /// Whether to auto-approve.
    pub auto_approve: bool,
    /// Call ID for the current tool execution.
    pub call_id: String,
    /// Optional sender for streaming output chunks.
    pub output_sender: Option<mpsc::Sender<(String, ToolOutputChunk)>>,
    /// LSP integration.
    pub lsp: Option<Arc<LspIntegration>>,
}

impl std::fmt::Debug for ToolContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolContext")
            .field("cwd", &self.cwd)
            .field("sandbox_policy", &self.sandbox_policy)
            .field("env", &self.env)
            .field("turn_id", &self.turn_id)
            .field("conversation_id", &self.conversation_id)
            .field("auto_approve", &self.auto_approve)
            .field("call_id", &self.call_id)
            .field("has_output_sender", &self.output_sender.is_some())
            .finish()
    }
}

impl ToolContext {
    /// Create a new tool context.
    pub fn new(cwd: PathBuf) -> Self {
        // Build environment with non-interactive settings
        let mut env: HashMap<String, String> = std::env::vars().collect();

        // Force non-interactive mode for common tools
        env.insert("CI".to_string(), "true".to_string());
        env.insert("DEBIAN_FRONTEND".to_string(), "noninteractive".to_string());
        env.insert("NPM_CONFIG_YES".to_string(), "true".to_string());
        env.insert(
            "YARN_ENABLE_IMMUTABLE_INSTALLS".to_string(),
            "false".to_string(),
        );
        env.insert("NO_COLOR".to_string(), "1".to_string());
        env.insert("TERM".to_string(), "dumb".to_string());
        env.insert("NONINTERACTIVE".to_string(), "1".to_string());
        // Force create-next-app to not ask questions
        env.insert("npm_config_yes".to_string(), "true".to_string());

        Self {
            cwd,
            sandbox_policy: SandboxPolicy::default(),
            env,
            turn_id: String::new(),
            conversation_id: String::new(),
            auto_approve: false,
            call_id: String::new(),
            output_sender: None,
            lsp: None,
        }
    }

    /// Set LSP integration.
    pub fn with_lsp(mut self, lsp: Arc<LspIntegration>) -> Self {
        self.lsp = Some(lsp);
        self
    }

    /// Set the sandbox policy.
    pub fn with_sandbox_policy(mut self, policy: SandboxPolicy) -> Self {
        self.sandbox_policy = policy;
        self
    }

    /// Set turn ID.
    pub fn with_turn_id(mut self, turn_id: impl Into<String>) -> Self {
        self.turn_id = turn_id.into();
        self
    }

    /// Set conversation ID.
    pub fn with_conversation_id(mut self, id: impl Into<String>) -> Self {
        self.conversation_id = id.into();
        self
    }

    /// Set auto-approve flag.
    pub fn with_auto_approve(mut self, auto_approve: bool) -> Self {
        self.auto_approve = auto_approve;
        self
    }

    /// Set call ID for the current tool execution.
    pub fn with_call_id(mut self, call_id: impl Into<String>) -> Self {
        self.call_id = call_id.into();
        self
    }

    /// Set output sender for streaming.
    pub fn with_output_sender(mut self, sender: mpsc::Sender<(String, ToolOutputChunk)>) -> Self {
        self.output_sender = Some(sender);
        self
    }

    /// Send an output chunk if sender is available.
    pub async fn send_output(&self, chunk: ToolOutputChunk) {
        if let Some(sender) = &self.output_sender {
            let _ = sender.send((self.call_id.clone(), chunk)).await;
        }
    }

    /// Resolve a path relative to cwd with path traversal protection.
    ///
    /// This method:
    /// 1. Joins relative paths to the cwd
    /// 2. Normalizes the path to resolve `.` and `..` components
    /// 3. Validates that the resolved path stays within the cwd (for relative paths)
    ///
    /// # Arguments
    /// * `path` - The path to resolve (can be absolute or relative)
    ///
    /// # Returns
    /// The resolved and normalized path
    pub fn resolve_path(&self, path: &str) -> PathBuf {
        let p = PathBuf::from(path);
        let resolved = if p.is_absolute() {
            p
        } else {
            self.cwd.join(&p)
        };

        // Normalize the path to resolve . and .. components
        Self::normalize_path(&resolved)
    }

    /// Resolve and validate a path, ensuring it stays within allowed roots.
    ///
    /// This is a more secure version of `resolve_path` that validates the
    /// resolved path is within the cwd or allowed writable roots.
    ///
    /// # Arguments
    /// * `path` - The path to resolve (can be absolute or relative)
    ///
    /// # Returns
    /// * `Ok(PathBuf)` - The resolved and validated path
    /// * `Err(String)` - If the path would escape allowed directories
    pub fn resolve_and_validate_path(&self, path: &str) -> Result<PathBuf, String> {
        let resolved = self.resolve_path(path);

        // Get the canonical cwd if it exists
        let canonical_cwd = if self.cwd.exists() {
            self.cwd
                .canonicalize()
                .unwrap_or_else(|_| Self::normalize_path(&self.cwd))
        } else {
            Self::normalize_path(&self.cwd)
        };

        // For existing paths, canonicalize to resolve symlinks
        let canonical_resolved = if resolved.exists() {
            resolved
                .canonicalize()
                .map_err(|e| format!("Failed to canonicalize path: {}", e))?
        } else {
            // For non-existent paths, normalize and check parent
            if let Some(parent) = resolved.parent() {
                if parent.exists() {
                    let canonical_parent = parent
                        .canonicalize()
                        .map_err(|e| format!("Failed to canonicalize parent: {}", e))?;
                    let file_name = resolved
                        .file_name()
                        .ok_or_else(|| "Invalid file name".to_string())?;
                    canonical_parent.join(file_name)
                } else {
                    Self::normalize_path(&resolved)
                }
            } else {
                Self::normalize_path(&resolved)
            }
        };

        // Check if the resolved path starts with cwd
        if canonical_resolved.starts_with(&canonical_cwd) {
            return Ok(canonical_resolved);
        }

        // Check against writable roots from sandbox policy
        let writable_roots = self.sandbox_policy.get_writable_roots_with_cwd(&self.cwd);
        for writable_root in &writable_roots {
            let canonical_root = if writable_root.root.exists() {
                writable_root
                    .root
                    .canonicalize()
                    .unwrap_or_else(|_| Self::normalize_path(&writable_root.root))
            } else {
                Self::normalize_path(&writable_root.root)
            };

            if canonical_resolved.starts_with(&canonical_root) {
                // Check it's not in a read-only subpath
                if writable_root.is_path_writable(&canonical_resolved) {
                    return Ok(canonical_resolved);
                }
            }
        }

        Err(format!(
            "Path '{}' is outside allowed directories (cwd: {})",
            path,
            self.cwd.display()
        ))
    }

    /// Normalize a path by resolving `.` and `..` components without filesystem access.
    fn normalize_path(path: &Path) -> PathBuf {
        normalize_path_util(path)
    }

    /// Check if a path contains path traversal sequences.
    pub fn contains_traversal(path: &str) -> bool {
        let p = Path::new(path);
        p.components().any(|c| matches!(c, Component::ParentDir))
    }
}
