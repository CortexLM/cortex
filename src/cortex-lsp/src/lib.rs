//! LSP (Language Server Protocol) support for Cortex CLI.
//!
//! Provides integration with language servers for:
//! - Diagnostics (errors, warnings)
//! - Hover information
//! - Go to definition
//! - Find references
//! - Document and workspace symbols
//! - Code completion
//! - Code actions and formatting
//! - Auto-downloading of language servers
//! - Multi-root workspace support (monorepos)

pub mod client;
pub mod diagnostics;
pub mod downloader;
pub mod manager;
pub mod markers;
pub mod root_detection;
pub mod server_config;
pub mod workspace;

pub use client::{CachedServerCapabilities, LspClient, LspClientConfig};
pub use diagnostics::{Diagnostic, DiagnosticSeverity};
pub use downloader::{DownloadableServer, InstallMethod, LspDownloader, ProgressCallback};
pub use manager::LspManager;
pub use markers::ProjectMarker;
pub use root_detection::{detect_root, DetectedRoot, NearestRoot, RootDetectionConfig};
pub use server_config::{LspServerConfig, BUILTIN_SERVERS};
pub use workspace::WorkspaceManager;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum LspError {
    #[error("LSP server not found: {0}")]
    ServerNotFound(String),
    #[error("Failed to start LSP server: {0}")]
    StartFailed(String),
    #[error("LSP communication error: {0}")]
    Communication(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Timeout waiting for LSP response")]
    Timeout,
}

pub type Result<T> = std::result::Result<T, LspError>;
