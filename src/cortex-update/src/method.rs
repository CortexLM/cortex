//! Installation method detection.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Installation method for Cortex CLI.
///
/// Supported methods:
/// - **macOS/Linux**: curl script, Homebrew
/// - **Windows**: PowerShell script, WinGet
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallMethod {
    /// curl -fsSL https://cortex.foundation/install.sh | bash (macOS/Linux)
    CurlScript,
    /// irm https://cortex.foundation/install.ps1 | iex (Windows)
    PowerShellScript,
    /// brew install --cask cortex (macOS/Linux)
    Homebrew,
    /// winget install Cortex.CLI (Windows)
    WinGet,
    /// Manual installation or unknown method
    Unknown,
}

impl InstallMethod {
    /// Detect the installation method based on environment and paths.
    pub fn detect() -> Self {
        // 1. Check executable path first
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(method) = Self::detect_from_path(&exe_path) {
                return method;
            }
        }

        // 2. Platform-specific defaults
        #[cfg(windows)]
        {
            // On Windows, check if WinGet is available
            if which_exists("winget") {
                return Self::WinGet;
            }
            return Self::PowerShellScript;
        }

        #[cfg(not(windows))]
        {
            // On macOS/Linux, check if Homebrew is available
            if which_exists("brew") {
                return Self::Homebrew;
            }
            return Self::CurlScript;
        }
    }

    /// Detect from executable path.
    fn detect_from_path(path: &PathBuf) -> Option<Self> {
        let path_str = path.to_string_lossy().to_lowercase();

        // Homebrew paths (macOS and Linux)
        if path_str.contains("/opt/homebrew/")
            || path_str.contains("/usr/local/cellar/")
            || path_str.contains("/home/linuxbrew/")
            || path_str.contains("/.linuxbrew/")
            || path_str.contains("/homebrew/caskroom/")
        {
            return Some(Self::Homebrew);
        }

        // WinGet paths (Windows)
        if path_str.contains("\\windowsapps\\") || path_str.contains("\\winget\\") {
            return Some(Self::WinGet);
        }

        // Cortex script install (default locations)
        // macOS/Linux: ~/.cortex/bin/cortex
        // Windows: %LOCALAPPDATA%\Cortex\bin\cortex.exe
        if path_str.contains("/.cortex/bin/")
            || path_str.contains("\\.cortex\\bin\\")
            || path_str.contains("\\cortex\\bin\\")
        {
            #[cfg(windows)]
            return Some(Self::PowerShellScript);
            #[cfg(not(windows))]
            return Some(Self::CurlScript);
        }

        None
    }

    /// Get the update command for this installation method.
    /// Returns None if self-update should be used instead.
    pub fn update_command(&self, _version: &str) -> Option<Vec<String>> {
        match self {
            Self::CurlScript => None,       // Use self-update binary replacement
            Self::PowerShellScript => None, // Use self-update binary replacement
            Self::Homebrew => Some(vec![
                "brew".to_string(),
                "upgrade".to_string(),
                "--cask".to_string(),
                "cortex".to_string(),
            ]),
            Self::WinGet => Some(vec![
                "winget".to_string(),
                "upgrade".to_string(),
                "Cortex.CLI".to_string(),
            ]),
            Self::Unknown => None,
        }
    }

    /// Check if this method supports self-update (direct binary replacement).
    pub fn supports_self_update(&self) -> bool {
        matches!(
            self,
            Self::CurlScript | Self::PowerShellScript | Self::Unknown
        )
    }

    /// Check if this method uses a package manager.
    pub fn uses_package_manager(&self) -> bool {
        matches!(self, Self::Homebrew | Self::WinGet)
    }

    /// Get a human-readable description of this method.
    pub fn description(&self) -> &'static str {
        match self {
            Self::CurlScript => "curl script (cortex.foundation)",
            Self::PowerShellScript => "PowerShell script (cortex.foundation)",
            Self::Homebrew => "Homebrew",
            Self::WinGet => "WinGet",
            Self::Unknown => "Manual/Unknown",
        }
    }

    /// Get the install hint for fresh installation.
    pub fn install_hint(&self) -> &'static str {
        match self {
            Self::CurlScript => "curl -fsSL https://cortex.foundation/install.sh | bash",
            Self::PowerShellScript => "irm https://cortex.foundation/install.ps1 | iex",
            Self::Homebrew => "brew install --cask cortex",
            Self::WinGet => "winget install Cortex.CLI",
            Self::Unknown => "Visit https://cortex.foundation/download",
        }
    }

    /// Get the update hint for manual update.
    pub fn update_hint(&self, version: &str) -> String {
        match self.update_command(version) {
            Some(cmd) => cmd.join(" "),
            None => self.install_hint().to_string(),
        }
    }
}

/// Check if a command exists in PATH.
fn which_exists(cmd: &str) -> bool {
    #[cfg(windows)]
    {
        std::process::Command::new("where")
            .arg(cmd)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("which")
            .arg(cmd)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

impl std::fmt::Display for InstallMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.description())
    }
}

impl Default for InstallMethod {
    fn default() -> Self {
        Self::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_from_path_homebrew() {
        let path = PathBuf::from("/opt/homebrew/bin/cortex");
        assert_eq!(
            InstallMethod::detect_from_path(&path),
            Some(InstallMethod::Homebrew)
        );

        let path = PathBuf::from("/usr/local/Cellar/cortex/0.1.0/bin/cortex");
        assert_eq!(
            InstallMethod::detect_from_path(&path),
            Some(InstallMethod::Homebrew)
        );
    }

    #[test]
    fn test_detect_from_path_winget() {
        let path = PathBuf::from("C:\\Program Files\\WindowsApps\\Cortex.CLI\\cortex.exe");
        assert_eq!(
            InstallMethod::detect_from_path(&path),
            Some(InstallMethod::WinGet)
        );
    }

    #[test]
    fn test_detect_from_path_curl_script() {
        let path = PathBuf::from("/home/user/.cortex/bin/cortex");
        #[cfg(not(windows))]
        assert_eq!(
            InstallMethod::detect_from_path(&path),
            Some(InstallMethod::CurlScript)
        );
    }

    #[test]
    fn test_update_command_homebrew() {
        let method = InstallMethod::Homebrew;
        let cmd = method.update_command("0.2.0");
        assert!(cmd.is_some());
        let cmd = cmd.unwrap();
        assert!(cmd.contains(&"brew".to_string()));
        assert!(cmd.contains(&"--cask".to_string()));
    }

    #[test]
    fn test_update_command_winget() {
        let method = InstallMethod::WinGet;
        let cmd = method.update_command("0.2.0");
        assert!(cmd.is_some());
        let cmd = cmd.unwrap();
        assert!(cmd.contains(&"winget".to_string()));
        assert!(cmd.contains(&"Cortex.CLI".to_string()));
    }

    #[test]
    fn test_supports_self_update() {
        assert!(InstallMethod::CurlScript.supports_self_update());
        assert!(InstallMethod::PowerShellScript.supports_self_update());
        assert!(!InstallMethod::Homebrew.supports_self_update());
        assert!(!InstallMethod::WinGet.supports_self_update());
    }
}
