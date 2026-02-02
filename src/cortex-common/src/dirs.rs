//! Cross-platform application directories for Cortex CLI.
//!
//! Provides unified directory management following platform conventions:
//! - Linux/macOS: `~/.cortex` (simple, easy to find)
//! - Windows: `%APPDATA%\cortex`
//!
//! Can be overridden with `CORTEX_HOME` environment variable.

use std::path::PathBuf;

/// Application name for directory paths
pub const APP_NAME: &str = "cortex";

/// Primary home directory name on Linux/macOS
pub const HOME_DIR_NAME: &str = ".cortex";

/// Legacy directory name (for migration from XDG paths)
pub const LEGACY_XDG_NAME: &str = ".config/cortex";

/// Application directories structure
#[derive(Debug, Clone)]
pub struct AppDirs {
    /// Configuration directory (~/.cortex on Linux/macOS, %APPDATA%\cortex on Windows)
    pub config_dir: PathBuf,
    /// Data directory (same as config_dir for simplicity)
    pub data_dir: PathBuf,
    /// Cache directory (~/.cortex/cache on Linux/macOS, %LOCALAPPDATA%\cortex on Windows)
    pub cache_dir: PathBuf,
    /// Legacy XDG home directory (~/.config/cortex) for migration
    pub legacy_xdg_home: PathBuf,
}

impl AppDirs {
    /// Get application directories, respecting environment variable overrides.
    ///
    /// Environment variables (in priority order):
    /// - `CORTEX_HOME`: Override all directories to this single path
    /// - `CORTEX_CONFIG_DIR`: Override config directory only
    /// - `CORTEX_DATA_DIR`: Override data directory only
    /// - `CORTEX_CACHE_DIR`: Override cache directory only
    ///
    /// Note: Relative paths in environment variables (like CORTEX_HOME=.)
    /// are automatically resolved to absolute paths to prevent config files
    /// being created in unexpected locations.
    pub fn new() -> Option<Self> {
        let home_dir = dirs::home_dir()?;

        // Check for CORTEX_HOME override first
        if let Ok(home) = std::env::var("CORTEX_HOME") {
            let home = PathBuf::from(&home);
            // Resolve relative paths to absolute to prevent unexpected behavior (#2008)
            // e.g., HOME=. would otherwise create config in current directory
            let home = if home.is_relative() {
                match std::env::current_dir() {
                    Ok(cwd) => {
                        let resolved = cwd.join(&home);
                        // Canonicalize if possible to resolve . and ..
                        resolved.canonicalize().unwrap_or(resolved)
                    }
                    Err(_) => {
                        // If we can't get cwd, fall back to home directory
                        eprintln!(
                            "Warning: CORTEX_HOME is a relative path ('{}') but current directory is unavailable. Using default location.",
                            home.display()
                        );
                        home_dir.join(HOME_DIR_NAME)
                    }
                }
            } else {
                home
            };
            return Some(Self {
                config_dir: home.clone(),
                data_dir: home.clone(),
                cache_dir: home.join("cache"),
                legacy_xdg_home: home_dir.join(LEGACY_XDG_NAME),
            });
        }

        // Platform-specific defaults
        #[cfg(target_os = "windows")]
        let (config_dir, data_dir, cache_dir) = {
            let appdata = dirs::config_dir()?; // %APPDATA%
            let local_appdata = dirs::cache_dir()?; // %LOCALAPPDATA%
            (
                appdata.join(APP_NAME),
                appdata.join(APP_NAME),
                local_appdata.join(APP_NAME),
            )
        };

        #[cfg(not(target_os = "windows"))]
        let (config_dir, data_dir, cache_dir) = {
            // Use ~/.cortex for Linux/macOS (simple, easy to find)
            let cortex_home = home_dir.join(HOME_DIR_NAME);
            (
                cortex_home.clone(),
                cortex_home.clone(),
                cortex_home.join("cache"),
            )
        };

        // Apply individual overrides
        let config_dir = std::env::var("CORTEX_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or(config_dir);
        let data_dir = std::env::var("CORTEX_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or(data_dir);
        let cache_dir = std::env::var("CORTEX_CACHE_DIR")
            .map(PathBuf::from)
            .unwrap_or(cache_dir);

        Some(Self {
            config_dir,
            data_dir,
            cache_dir,
            legacy_xdg_home: home_dir.join(LEGACY_XDG_NAME),
        })
    }

    /// Get the primary config file path (config.toml)
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    /// Get the auth storage directory
    pub fn auth_dir(&self) -> PathBuf {
        self.data_dir.join("auth")
    }

    /// Get the sessions directory
    pub fn sessions_dir(&self) -> PathBuf {
        self.data_dir.join("sessions")
    }

    /// Get the agents directory
    pub fn agents_dir(&self) -> PathBuf {
        self.data_dir.join("agents")
    }

    /// Get the logs directory
    pub fn logs_dir(&self) -> PathBuf {
        self.cache_dir.join("logs")
    }

    /// Get the MCP servers directory
    pub fn mcps_dir(&self) -> PathBuf {
        self.data_dir.join("mcps")
    }

    /// Ensure all directories exist with proper permissions
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        for dir in [&self.config_dir, &self.data_dir, &self.cache_dir] {
            if !dir.exists() {
                std::fs::create_dir_all(dir)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
                }
            }
        }
        Ok(())
    }

    /// Check if legacy XDG directories exist and need migration
    pub fn has_legacy_data(&self) -> bool {
        self.legacy_xdg_home.exists()
    }

    /// Get the effective home directory (for backward compatibility)
    /// Prefers new ~/.cortex location, falls back to legacy XDG if it exists
    pub fn effective_home(&self) -> PathBuf {
        if self.config_dir.exists() {
            self.config_dir.clone()
        } else if self.legacy_xdg_home.exists() {
            self.legacy_xdg_home.clone()
        } else {
            self.config_dir.clone()
        }
    }
}

impl Default for AppDirs {
    fn default() -> Self {
        Self::new().expect("Failed to determine application directories")
    }
}

/// Get application directories (convenience function)
pub fn get_app_dirs() -> Option<AppDirs> {
    AppDirs::new()
}

/// Get the effective home directory for the application
/// This is the main entry point for backward compatibility
pub fn get_cortex_home() -> Option<PathBuf> {
    let dirs = AppDirs::new()?;
    Some(dirs.effective_home())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_dirs_creation() {
        let dirs = AppDirs::new();
        assert!(dirs.is_some());
    }

    #[test]
    fn test_config_file_path() {
        let dirs = AppDirs::new().unwrap();
        assert!(dirs.config_file().ends_with("config.toml"));
    }

    #[test]
    fn test_env_override() {
        // Use a cross-platform temp directory path for the test
        let test_path = std::env::temp_dir().join("test-cortex");
        // SAFETY: This test runs in isolation and only modifies test-specific env vars
        unsafe {
            std::env::set_var("CORTEX_HOME", &test_path);
        }
        let dirs = AppDirs::new().unwrap();
        assert_eq!(dirs.config_dir, test_path);
        unsafe {
            std::env::remove_var("CORTEX_HOME");
        }
    }
}
