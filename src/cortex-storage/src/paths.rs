//! OS-aware path detection for Cortex storage.
//!
//! Provides automatic detection of the appropriate storage location
//! based on the operating system:
//!
//! - **Windows**: `%APPDATA%\Cortex\` (e.g., `C:\Users\<user>\AppData\Roaming\Cortex\`)
//! - **macOS**: `~/Library/Application Support/Cortex/`
//! - **Linux**: `~/.local/share/Cortex/`
//!
//! ## Issue #2324: Docker --read-only Container Support
//!
//! When running Cortex in a Docker container with `--read-only` filesystem flag,
//! the following directories must be mounted as writable volumes:
//!
//! **Required writable directories:**
//! 1. **Data directory** (session storage, history):
//!    - Linux: `~/.local/share/Cortex/` or `$CORTEX_DATA_DIR`
//!    - macOS: `~/Library/Application Support/Cortex/`
//!    - Windows: `%APPDATA%\Cortex\`
//!
//! 2. **Config directory** (configuration files):
//!    - Linux: `~/.config/Cortex/` or legacy `~/.cortex/`
//!    - macOS: `~/Library/Application Support/Cortex/`
//!    - Windows: `%APPDATA%\Cortex\`
//!
//! 3. **Cache directory** (temporary files):
//!    - Linux: `~/.cache/Cortex/` or `$XDG_CACHE_HOME/Cortex/`
//!    - macOS: `~/Library/Caches/Cortex/`
//!    - Windows: `%LOCALAPPDATA%\Cortex\Cache\`
//!
//! **Example Docker command:**
//! ```bash
//! docker run --read-only \
//!   -v /host/cortex-data:/home/user/.local/share/Cortex \
//!   -v /host/cortex-config:/home/user/.config/Cortex \
//!   -v /host/cortex-cache:/home/user/.cache/Cortex \
//!   -v /tmp:/tmp:rw \
//!   cortex:latest
//! ```
//!
//! **Note:** Cortex may also use system temp directories (`/tmp` or `$TMPDIR`)
//! for ephemeral files. Mount these as writable if needed.

use std::path::PathBuf;
use tracing::debug;

use crate::error::{Result, StorageError};

/// Application name used for storage directories.
pub const APP_NAME: &str = "Cortex";

/// Subdirectory names.
pub const SESSIONS_DIR: &str = "sessions";
pub const HISTORY_DIR: &str = "history";
pub const CACHE_DIR: &str = "cache";
pub const LOGS_DIR: &str = "logs";
pub const CONFIG_FILE: &str = "config.toml";

/// Cortex storage paths container.
#[derive(Debug, Clone)]
pub struct CortexPaths {
    /// Root data directory (platform-specific).
    pub data_dir: PathBuf,
    /// Sessions metadata directory.
    pub sessions_dir: PathBuf,
    /// Message history directory.
    pub history_dir: PathBuf,
    /// Cache directory.
    pub cache_dir: PathBuf,
    /// Logs directory.
    pub logs_dir: PathBuf,
}

impl CortexPaths {
    /// Create CortexPaths with automatic OS detection.
    pub fn new() -> Result<Self> {
        let data_dir = cortex_data_dir()?;
        Ok(Self::from_root(data_dir))
    }

    /// Create CortexPaths from a custom root directory.
    pub fn from_root(data_dir: PathBuf) -> Self {
        Self {
            sessions_dir: data_dir.join(SESSIONS_DIR),
            history_dir: data_dir.join(HISTORY_DIR),
            cache_dir: data_dir.join(CACHE_DIR),
            logs_dir: data_dir.join(LOGS_DIR),
            data_dir,
        }
    }

    /// Ensure all directories exist.
    pub fn ensure_dirs(&self) -> Result<()> {
        std::fs::create_dir_all(&self.data_dir)?;
        std::fs::create_dir_all(&self.sessions_dir)?;
        std::fs::create_dir_all(&self.history_dir)?;
        std::fs::create_dir_all(&self.cache_dir)?;
        std::fs::create_dir_all(&self.logs_dir)?;
        debug!(data_dir = %self.data_dir.display(), "Cortex storage directories initialized");
        Ok(())
    }

    /// Ensure all directories exist (async version).
    pub async fn ensure_dirs_async(&self) -> Result<()> {
        tokio::fs::create_dir_all(&self.data_dir).await?;
        tokio::fs::create_dir_all(&self.sessions_dir).await?;
        tokio::fs::create_dir_all(&self.history_dir).await?;
        tokio::fs::create_dir_all(&self.cache_dir).await?;
        tokio::fs::create_dir_all(&self.logs_dir).await?;
        debug!(data_dir = %self.data_dir.display(), "Cortex storage directories initialized");
        Ok(())
    }

    /// Get path for a session metadata file.
    pub fn session_path(&self, session_id: &str) -> PathBuf {
        self.sessions_dir.join(format!("{}.json", session_id))
    }

    /// Get path for a session history file.
    pub fn history_path(&self, session_id: &str) -> PathBuf {
        self.history_dir.join(format!("{}.jsonl", session_id))
    }
}

impl Default for CortexPaths {
    fn default() -> Self {
        Self::new().expect("Failed to determine Cortex storage paths")
    }
}

/// Get the Cortex data directory based on the current OS.
///
/// Returns:
/// - **Windows**: `%APPDATA%\Cortex\`
/// - **macOS**: `~/Library/Application Support/Cortex/`
/// - **Linux**: `~/.local/share/Cortex/`
pub fn cortex_data_dir() -> Result<PathBuf> {
    // Check environment variable override first
    if let Ok(val) = std::env::var("CORTEX_DATA_DIR") {
        if !val.is_empty() {
            let path = PathBuf::from(val);
            debug!(path = %path.display(), "Using CORTEX_DATA_DIR override");
            return Ok(path);
        }
    }

    // Use platform-specific data directory
    let base = dirs::data_dir().ok_or(StorageError::HomeDirNotFound)?;
    Ok(base.join(APP_NAME))
}

/// Get the Cortex config directory based on the current OS.
///
/// Returns:
/// - **Windows**: `%APPDATA%\Cortex\`
/// - **macOS**: `~/Library/Application Support/Cortex/`
/// - **Linux**: `~/.config/Cortex/`
pub fn cortex_config_dir() -> Result<PathBuf> {
    // Check environment variable override first
    if let Ok(val) = std::env::var("CORTEX_CONFIG_DIR") {
        if !val.is_empty() {
            let path = PathBuf::from(val);
            debug!(path = %path.display(), "Using CORTEX_CONFIG_DIR override");
            return Ok(path);
        }
    }

    // Use platform-specific config directory
    let base = dirs::config_dir().ok_or(StorageError::HomeDirNotFound)?;
    Ok(base.join(APP_NAME))
}

/// Get the legacy Cortex home directory (~/.cortex).
///
/// This is kept for backwards compatibility during migration.
pub fn legacy_cortex_home() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or(StorageError::HomeDirNotFound)?;
    Ok(home.join(".cortex"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cortex_data_dir() {
        let path = cortex_data_dir().unwrap();
        assert!(path.ends_with(APP_NAME));
    }

    #[test]
    fn test_cortex_paths_structure() {
        let paths = CortexPaths::new().unwrap();
        assert!(paths.sessions_dir.ends_with(SESSIONS_DIR));
        assert!(paths.history_dir.ends_with(HISTORY_DIR));
        assert!(paths.cache_dir.ends_with(CACHE_DIR));
        assert!(paths.logs_dir.ends_with(LOGS_DIR));
    }

    #[test]
    fn test_session_path() {
        let paths = CortexPaths::new().unwrap();
        let session_path = paths.session_path("test-session-123");
        assert!(session_path
            .to_string_lossy()
            .contains("test-session-123.json"));
    }

    #[test]
    fn test_history_path() {
        let paths = CortexPaths::new().unwrap();
        let history_path = paths.history_path("test-session-123");
        assert!(history_path
            .to_string_lossy()
            .contains("test-session-123.jsonl"));
    }
}
