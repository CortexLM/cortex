#![allow(warnings, clippy::all)]
//! Cortex Update - Auto-update system for Cortex CLI
//!
//! Provides automatic update checking and installation via:
//! - Cortex Foundation software distribution API
//! - Multiple installation method detection (npm, brew, choco, etc.)
//! - Cross-platform binary replacement
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_update::{UpdateManager, UpdateConfig};
//!
//! let manager = UpdateManager::new()?;
//!
//! // Check for updates
//! if let Some(info) = manager.check_update().await? {
//!     println!("Update available: {} -> {}", info.current_version, info.latest_version);
//!     
//!     // Download and install
//!     let download = manager.download_update(&info).await?;
//!     manager.verify(&download)?;
//!     manager.install(&download).await?;
//! }
//! ```

mod api;
mod config;
mod download;
mod error;
mod install;
mod manager;
mod method;
mod verify;
mod version;

pub use api::{CortexSoftwareClient, ReleaseAsset, ReleaseInfo};
pub use config::{ReleaseChannel, UpdateConfig, UpdateMode};
pub use error::{UpdateError, UpdateResult};
pub use install::DownloadedUpdate;
pub use manager::{UpdateInfo, UpdateManager, UpdateOutcome};
pub use method::InstallMethod;
pub use version::VersionCache;

/// Current version of Cortex CLI (set at compile time)
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default software distribution URL
pub const SOFTWARE_URL: &str = "https://software.cortex.foundation";
