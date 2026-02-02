//! Integration modules for new feature crates.
//!
//! This module provides unified access to all the new features:
//! - LSP integration for diagnostics
//! - Ghost commits for undo
//! - Session resume
//! - Rate limits tracking
//! - Model migrations
//! - Experimental features

pub mod experimental_integration;
pub mod ghost_integration;
pub mod lsp_integration;
pub mod migration_integration;
pub mod ratelimit_integration;
pub mod resume_integration;

// Re-export key types
pub use experimental_integration::ExperimentalIntegration;
pub use ghost_integration::GhostIntegration;
pub use lsp_integration::LspIntegration;
pub use migration_integration::MigrationIntegration;
pub use ratelimit_integration::RatelimitIntegration;
pub use resume_integration::ResumeIntegration;
