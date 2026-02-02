//! LSP server downloader for auto-downloading language servers from GitHub releases.

#![allow(
    clippy::collapsible_else_if,
    clippy::redundant_closure,
    clippy::useless_format,
    clippy::double_ended_iterator_last,
    clippy::io_other_error
)]

pub mod archive;
pub mod core;
pub mod http;
pub mod servers;
#[cfg(test)]
mod tests;
pub mod types;

// Re-export main types for backward compatibility
pub use core::LspDownloader;
pub use types::{DownloadableServer, InstallMethod, ProgressCallback};
