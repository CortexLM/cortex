//! Shell type detection and configuration.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Supported shell types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShellType {
    /// Z Shell (zsh).
    Zsh,

    /// Bourne Again Shell (bash).
    Bash,

    /// POSIX Shell (sh, dash, ash).
    Sh,

    /// Fish shell.
    Fish,

    /// PowerShell.
    PowerShell,
}

impl ShellType {
    /// Detect shell type from environment.
    pub fn detect() -> Option<Self> {
        // Check SHELL environment variable
        if let Ok(shell) = std::env::var("SHELL") {
            return Self::from_path(&shell);
        }

        // Fallback to checking parent process name
        None
    }

    /// Parse shell type from a path.
    pub fn from_path(path: &str) -> Option<Self> {
        let path = Path::new(path);
        let name = path.file_name()?.to_str()?;

        Self::from_name(name)
    }

    /// Parse shell type from a name.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "zsh" => Some(ShellType::Zsh),
            "bash" => Some(ShellType::Bash),
            "sh" | "dash" | "ash" => Some(ShellType::Sh),
            "fish" => Some(ShellType::Fish),
            "pwsh" | "powershell" | "powershell.exe" => Some(ShellType::PowerShell),
            _ => None,
        }
    }

    /// Get the shell name.
    pub fn name(&self) -> &str {
        match self {
            ShellType::Zsh => "zsh",
            ShellType::Bash => "bash",
            ShellType::Sh => "sh",
            ShellType::Fish => "fish",
            ShellType::PowerShell => "pwsh",
        }
    }

    /// Get the default path to the shell.
    pub fn default_path(&self) -> &str {
        match self {
            ShellType::Zsh => "/bin/zsh",
            ShellType::Bash => "/bin/bash",
            ShellType::Sh => "/bin/sh",
            ShellType::Fish => "/usr/bin/fish",
            ShellType::PowerShell => "pwsh",
        }
    }

    /// Get the rc file name.
    pub fn rc_file(&self) -> Option<&str> {
        match self {
            ShellType::Zsh => Some(".zshrc"),
            ShellType::Bash => Some(".bashrc"),
            ShellType::Sh => None, // sh doesn't have a standard rc file
            ShellType::Fish => Some("config.fish"),
            ShellType::PowerShell => None,
        }
    }

    /// Check if the shell supports function export.
    pub fn supports_function_export(&self) -> bool {
        matches!(self, ShellType::Bash | ShellType::Zsh)
    }

    /// Check if the shell supports aliases.
    pub fn supports_aliases(&self) -> bool {
        !matches!(self, ShellType::PowerShell)
    }

    /// Check if snapshotting is supported for this shell.
    pub fn supports_snapshot(&self) -> bool {
        matches!(self, ShellType::Zsh | ShellType::Bash | ShellType::Sh)
    }

    /// Get file extension for snapshot files.
    pub fn snapshot_extension(&self) -> &str {
        match self {
            ShellType::Zsh => "zsh",
            ShellType::Bash => "bash",
            ShellType::Sh => "sh",
            ShellType::Fish => "fish",
            ShellType::PowerShell => "ps1",
        }
    }
}

impl std::fmt::Display for ShellType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl std::str::FromStr for ShellType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_name(s).ok_or_else(|| format!("Unknown shell type: {}", s))
    }
}

impl Default for ShellType {
    fn default() -> Self {
        Self::detect().unwrap_or(ShellType::Bash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_path() {
        assert_eq!(ShellType::from_path("/bin/zsh"), Some(ShellType::Zsh));
        assert_eq!(ShellType::from_path("/usr/bin/bash"), Some(ShellType::Bash));
        assert_eq!(ShellType::from_path("/bin/sh"), Some(ShellType::Sh));
    }

    #[test]
    fn test_from_name() {
        assert_eq!(ShellType::from_name("zsh"), Some(ShellType::Zsh));
        assert_eq!(ShellType::from_name("BASH"), Some(ShellType::Bash));
        assert_eq!(ShellType::from_name("dash"), Some(ShellType::Sh));
    }

    #[test]
    fn test_supports_snapshot() {
        assert!(ShellType::Zsh.supports_snapshot());
        assert!(ShellType::Bash.supports_snapshot());
        assert!(ShellType::Sh.supports_snapshot());
        assert!(!ShellType::Fish.supports_snapshot());
        assert!(!ShellType::PowerShell.supports_snapshot());
    }
}
