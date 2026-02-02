//! Configuration loading utilities.
//!
//! Supports multiple configuration formats:
//! - TOML (`.toml`) - Primary format
//! - JSON with comments (`.json`, `.jsonc`) - JSONC format

use std::path::{Path, PathBuf};

use tracing::debug;

use super::project_config::{
    find_project_config, load_project_config, load_project_config_async, merge_configs,
};
use super::types::ConfigToml;

/// Configuration file name (TOML).
pub const CONFIG_FILE: &str = "config.toml";

/// Configuration file name (JSON).
pub const CONFIG_FILE_JSON: &str = "config.json";

/// Configuration file name (JSONC).
pub const CONFIG_FILE_JSONC: &str = "config.jsonc";

/// Environment variable for custom config file path.
pub const CORTEX_CONFIG_ENV: &str = "CORTEX_CONFIG";

/// Environment variable for custom config directory.
pub const CORTEX_CONFIG_DIR_ENV: &str = "CORTEX_CONFIG_DIR";

/// Find the Cortex home directory.
///
/// Checks in order:
/// 1. `CORTEX_CONFIG_DIR` environment variable
/// 2. `CORTEX_HOME` environment variable
/// 3. Default `~/.cortex` directory (respecting SUDO_USER if running as root)
///
/// When running with sudo, this function detects SUDO_USER and uses that
/// user's home directory instead of /root/.cortex to prevent creating
/// root-owned config files in the user's home directory.
pub fn find_cortex_home() -> std::io::Result<PathBuf> {
    // Check CORTEX_CONFIG_DIR environment variable first (new)
    if let Ok(val) = std::env::var(CORTEX_CONFIG_DIR_ENV) {
        if !val.is_empty() {
            let path = PathBuf::from(&val);
            debug!(path = %path.display(), "Using CORTEX_CONFIG_DIR");
            return Ok(path);
        }
    }

    // Check CORTEX_HOME environment variable
    if let Ok(val) = std::env::var("CORTEX_HOME") {
        if !val.is_empty() {
            let path = PathBuf::from(&val);
            debug!(path = %path.display(), "Using CORTEX_HOME");
            return Ok(path);
        }
    }

    // Default to ~/.cortex, but respect SUDO_USER if running as root
    let home = get_effective_home_dir()?;
    let cortex_home = home.join(".cortex");
    Ok(cortex_home)
}

/// Get the effective home directory, respecting SUDO_USER when running as root.
///
/// This prevents creating root-owned config files in the user's home directory
/// when running cortex with sudo.
fn get_effective_home_dir() -> std::io::Result<PathBuf> {
    // Check if we're running as root (uid 0 on Unix)
    #[cfg(unix)]
    {
        // Check if effective user is root
        let is_root = unsafe { libc::geteuid() } == 0;

        if is_root {
            // Check for SUDO_USER to get the original user's home
            if let Ok(sudo_user) = std::env::var("SUDO_USER") {
                if !sudo_user.is_empty() && sudo_user != "root" {
                    // Try to get the user's home directory from /etc/passwd
                    if let Some(user_home) = get_user_home_dir(&sudo_user) {
                        debug!(
                            user = %sudo_user,
                            home = %user_home.display(),
                            "Using SUDO_USER's home directory"
                        );
                        return Ok(user_home);
                    }
                }
            }

            // Also check for SUDO_UID
            if let Ok(sudo_uid) = std::env::var("SUDO_UID") {
                if let Ok(uid) = sudo_uid.parse::<u32>() {
                    if uid != 0 {
                        if let Some(user_home) = get_user_home_dir_by_uid(uid) {
                            debug!(
                                uid = uid,
                                home = %user_home.display(),
                                "Using SUDO_UID's home directory"
                            );
                            return Ok(user_home);
                        }
                    }
                }
            }

            // Warn if running as root without SUDO_USER
            debug!("Running as root without SUDO_USER, using root's home directory");
        }
    }

    // Fall back to normal home directory detection
    dirs::home_dir().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "Home directory not found")
    })
}

/// Get a user's home directory by username.
#[cfg(unix)]
fn get_user_home_dir(username: &str) -> Option<PathBuf> {
    use std::ffi::CString;

    let username_c = CString::new(username).ok()?;
    let passwd = unsafe { libc::getpwnam(username_c.as_ptr()) };

    if passwd.is_null() {
        return None;
    }

    let home_dir = unsafe {
        let home = (*passwd).pw_dir;
        if home.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr(home).to_str().ok()?.to_string()
    };

    Some(PathBuf::from(home_dir))
}

/// Get a user's home directory by UID.
#[cfg(unix)]
fn get_user_home_dir_by_uid(uid: u32) -> Option<PathBuf> {
    let passwd = unsafe { libc::getpwuid(uid) };

    if passwd.is_null() {
        return None;
    }

    let home_dir = unsafe {
        let home = (*passwd).pw_dir;
        if home.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr(home).to_str().ok()?.to_string()
    };

    Some(PathBuf::from(home_dir))
}

#[cfg(not(unix))]
fn get_user_home_dir(_username: &str) -> Option<PathBuf> {
    None
}

#[cfg(not(unix))]
fn get_user_home_dir_by_uid(_uid: u32) -> Option<PathBuf> {
    None
}

/// Get the config file path, checking CORTEX_CONFIG env var first.
///
/// Returns the path to the config file to load. If `CORTEX_CONFIG` is set,
/// returns that path directly. Otherwise, returns `{cortex_home}/config.toml`.
pub fn get_config_path(cortex_home: &Path) -> PathBuf {
    // Check CORTEX_CONFIG environment variable
    if let Ok(val) = std::env::var(CORTEX_CONFIG_ENV) {
        if !val.is_empty() {
            let path = PathBuf::from(&val);
            debug!(path = %path.display(), "Using CORTEX_CONFIG");
            return path;
        }
    }

    cortex_home.join(CONFIG_FILE)
}

/// Load global configuration from disk.
pub async fn load_config(cortex_home: &Path) -> std::io::Result<ConfigToml> {
    let config_path = get_config_path(cortex_home);
    load_config_from_path(&config_path).await
}

/// Load configuration from a specific path.
///
/// Supports both TOML and JSONC formats based on file extension.
async fn load_config_from_path(config_path: &Path) -> std::io::Result<ConfigToml> {
    if !config_path.exists() {
        debug!(path = %config_path.display(), "Config file not found, using defaults");
        return Ok(ConfigToml::default());
    }

    debug!(path = %config_path.display(), "Loading config file");
    let content = tokio::fs::read_to_string(config_path).await?;
    let format = ConfigFormat::from_path(config_path);

    parse_config_content(&content, format).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Failed to parse {}: {e}", config_path.display()),
        )
    })
}

/// Load configuration synchronously.
pub fn load_config_sync(cortex_home: &Path) -> std::io::Result<ConfigToml> {
    let config_path = get_config_path(cortex_home);
    load_config_from_path_sync(&config_path)
}

/// Load configuration from a specific path synchronously.
///
/// Supports both TOML and JSONC formats based on file extension.
fn load_config_from_path_sync(config_path: &Path) -> std::io::Result<ConfigToml> {
    if !config_path.exists() {
        debug!(path = %config_path.display(), "Config file not found, using defaults");
        return Ok(ConfigToml::default());
    }

    debug!(path = %config_path.display(), "Loading config file");
    let content = std::fs::read_to_string(config_path)?;
    let format = ConfigFormat::from_path(config_path);

    parse_config_content(&content, format).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Failed to parse {}: {e}", config_path.display()),
        )
    })
}

/// Load merged configuration (global + project).
///
/// This is the main entry point for loading configuration with project-level
/// overrides. It:
/// 1. Loads the global config from `~/.cortex/config.toml`
/// 2. Discovers and loads project config (`.cortex/config.toml` or `cortex.toml`)
/// 3. Merges them with project taking precedence over global
///
/// # Arguments
/// * `cortex_home` - The global Cortex home directory
/// * `cwd` - Current working directory for project config discovery
///
/// # Returns
/// * `Ok((ConfigToml, Option<PathBuf>))` - Merged config and optional project config path
pub async fn load_merged_config(
    cortex_home: &Path,
    cwd: &Path,
) -> std::io::Result<(ConfigToml, Option<PathBuf>)> {
    // Load global config
    let global_config = load_config(cortex_home).await?;

    // Try to find and load project config
    let project_config_path = find_project_config(cwd);
    let project_config = if let Some(ref path) = project_config_path {
        match load_project_config_async(path).await {
            Ok(config) => Some(config),
            Err(e) => {
                debug!(path = %path.display(), error = %e, "Failed to load project config");
                None
            }
        }
    } else {
        None
    };

    // Merge configs
    let merged = merge_configs(global_config, project_config);

    Ok((merged, project_config_path))
}

/// Load merged configuration synchronously.
pub fn load_merged_config_sync(
    cortex_home: &Path,
    cwd: &Path,
) -> std::io::Result<(ConfigToml, Option<PathBuf>)> {
    // Load global config
    let global_config = load_config_sync(cortex_home)?;

    // Try to find and load project config
    let project_config_path = find_project_config(cwd);
    let project_config = if let Some(ref path) = project_config_path {
        match load_project_config(path) {
            Ok(config) => Some(config),
            Err(e) => {
                debug!(path = %path.display(), error = %e, "Failed to load project config");
                None
            }
        }
    } else {
        None
    };

    // Merge configs
    let merged = merge_configs(global_config, project_config);

    Ok((merged, project_config_path))
}

/// Ensure cortex home directory exists.
#[allow(dead_code)]
pub fn ensure_cortex_home(cortex_home: &Path) -> std::io::Result<()> {
    if !cortex_home.exists() {
        std::fs::create_dir_all(cortex_home)?;
    }
    Ok(())
}

/// Get the sessions directory.
#[allow(dead_code)]
pub fn sessions_dir(cortex_home: &Path) -> PathBuf {
    cortex_home.join("sessions")
}

/// Get the log directory.
#[allow(dead_code)]
pub fn log_dir(cortex_home: &Path) -> PathBuf {
    cortex_home.join("log")
}

/// Detected configuration format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFormat {
    /// TOML format (.toml)
    Toml,
    /// JSON with comments (.json, .jsonc)
    JsonC,
}

impl ConfigFormat {
    /// Detect format from file extension.
    pub fn from_path(path: &Path) -> Self {
        match path.extension().and_then(|e| e.to_str()) {
            Some("json") | Some("jsonc") => Self::JsonC,
            _ => Self::Toml, // Default to TOML
        }
    }
}

/// Strip C-style and C++ style comments from JSON content.
///
/// Handles:
/// - Single-line comments: `// comment`
/// - Multi-line comments: `/* comment */`
/// - Preserves comments inside strings
pub fn strip_json_comments(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escape_next = false;

    while let Some(c) = chars.next() {
        if escape_next {
            result.push(c);
            escape_next = false;
            continue;
        }

        if in_string {
            result.push(c);
            if c == '\\' {
                escape_next = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }

        match c {
            '"' => {
                in_string = true;
                result.push(c);
            }
            '/' => {
                match chars.peek() {
                    Some('/') => {
                        // Single-line comment - skip until newline
                        chars.next();
                        while let Some(&next) = chars.peek() {
                            if next == '\n' {
                                break;
                            }
                            chars.next();
                        }
                    }
                    Some('*') => {
                        // Multi-line comment - skip until */
                        chars.next();
                        let mut found_star = false;
                        while let Some(next) = chars.next() {
                            if found_star && next == '/' {
                                break;
                            }
                            found_star = next == '*';
                        }
                    }
                    _ => {
                        result.push(c);
                    }
                }
            }
            _ => {
                result.push(c);
            }
        }
    }

    result
}

/// Parse configuration content based on format.
pub fn parse_config_content(content: &str, format: ConfigFormat) -> std::io::Result<ConfigToml> {
    match format {
        ConfigFormat::Toml => toml::from_str(content).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format_toml_error(&e, content),
            )
        }),
        ConfigFormat::JsonC => {
            let stripped = strip_json_comments(content);
            serde_json::from_str(&stripped).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format_json_error(&e, &stripped),
                )
            })
        }
    }
}

/// Format a TOML parse error with user-friendly context.
fn format_toml_error(e: &toml::de::Error, content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let mut msg = String::from("Configuration error in TOML file:\n");

    // Try to extract line/column information
    let err_str = e.to_string();
    if let Some(span) = e.span() {
        // Calculate line number from byte offset
        let mut line_num = 1;
        let mut col = 1;
        for (i, c) in content.chars().enumerate() {
            if i >= span.start {
                break;
            }
            if c == '\n' {
                line_num += 1;
                col = 1;
            } else {
                col += 1;
            }
        }

        msg.push_str(&format!("  Error at line {}, column {}:\n", line_num, col));

        // Show the problematic line if available
        if line_num > 0 && line_num <= lines.len() {
            let line_content = lines[line_num - 1];
            msg.push_str(&format!("    {}: {}\n", line_num, line_content));
            // Show pointer to the column
            if col > 0 {
                msg.push_str(&format!(
                    "    {}  {}\n",
                    " ".repeat(line_num.to_string().len()),
                    " ".repeat(col - 1) + "^"
                ));
            }
        }
    }

    msg.push_str(&format!("  Problem: {}\n", err_str));
    msg.push_str("  Hint: Check for missing quotes, unclosed brackets, or invalid syntax.");
    msg
}

/// Format a JSON parse error with user-friendly context.
fn format_json_error(e: &serde_json::Error, content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let mut msg = String::from("Configuration error in JSON file:\n");

    let line_num = e.line();
    let col = e.column();

    msg.push_str(&format!("  Error at line {}, column {}:\n", line_num, col));

    // Show the problematic line if available
    if line_num > 0 && line_num <= lines.len() {
        let line_content = lines[line_num - 1];
        msg.push_str(&format!("    {}: {}\n", line_num, line_content));
        // Show pointer to the column
        if col > 0 {
            msg.push_str(&format!(
                "    {}  {}\n",
                " ".repeat(line_num.to_string().len()),
                " ".repeat(col.saturating_sub(1)) + "^"
            ));
        }
    }

    msg.push_str(&format!("  Problem: {}\n", e));
    msg.push_str("  Hint: Check for missing commas, unclosed braces, or trailing commas.");
    msg
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_load_missing_config() {
        let temp_dir = TempDir::new().unwrap();
        let config = load_config(temp_dir.path()).await.unwrap();
        assert!(config.model.is_none());
    }

    #[tokio::test]
    async fn test_load_valid_config() {
        let temp_dir = TempDir::new().unwrap();
        let config_content = r#"
model = "gpt-4o"
model_provider = "openai"
approval_policy = "on-request"
"#;
        tokio::fs::write(temp_dir.path().join(CONFIG_FILE), config_content)
            .await
            .unwrap();

        let config = load_config(temp_dir.path()).await.unwrap();
        assert_eq!(config.model, Some("gpt-4o".to_string()));
    }

    #[tokio::test]
    async fn test_load_merged_config_global_only() {
        let temp_dir = TempDir::new().unwrap();
        let global_home = temp_dir.path().join("global");
        std::fs::create_dir_all(&global_home).unwrap();

        let project_dir = temp_dir.path().join("project");
        std::fs::create_dir_all(&project_dir).unwrap();

        // Write global config
        let global_config = r#"
model = "global-model"
model_provider = "global-provider"
"#;
        std::fs::write(global_home.join(CONFIG_FILE), global_config).unwrap();

        let (config, project_path) = load_merged_config(&global_home, &project_dir)
            .await
            .unwrap();

        assert_eq!(config.model, Some("global-model".to_string()));
        assert_eq!(config.model_provider, Some("global-provider".to_string()));
        assert!(project_path.is_none());
    }

    #[tokio::test]
    async fn test_load_merged_config_with_project() {
        let temp_dir = TempDir::new().unwrap();
        let global_home = temp_dir.path().join("global");
        std::fs::create_dir_all(&global_home).unwrap();

        let project_dir = temp_dir.path().join("project");
        let cortex_dir = project_dir.join(".cortex");
        std::fs::create_dir_all(&cortex_dir).unwrap();

        // Write global config
        let global_config = r#"
model = "global-model"
model_provider = "global-provider"
"#;
        std::fs::write(global_home.join(CONFIG_FILE), global_config).unwrap();

        // Write project config
        let project_config = r#"
model = "project-model"
"#;
        std::fs::write(cortex_dir.join(CONFIG_FILE), project_config).unwrap();

        let (config, project_path) = load_merged_config(&global_home, &project_dir)
            .await
            .unwrap();

        // Project model should override global
        assert_eq!(config.model, Some("project-model".to_string()));
        // Global provider should be preserved
        assert_eq!(config.model_provider, Some("global-provider".to_string()));
        assert!(project_path.is_some());
    }

    #[test]
    fn test_load_merged_config_sync() {
        let temp_dir = TempDir::new().unwrap();
        let global_home = temp_dir.path().join("global");
        std::fs::create_dir_all(&global_home).unwrap();

        let project_dir = temp_dir.path().join("project");
        std::fs::create_dir_all(&project_dir).unwrap();

        // Write global config
        let global_config = r#"
model = "sync-model"
"#;
        std::fs::write(global_home.join(CONFIG_FILE), global_config).unwrap();

        let (config, _) = load_merged_config_sync(&global_home, &project_dir).unwrap();
        assert_eq!(config.model, Some("sync-model".to_string()));
    }

    #[test]
    fn test_get_config_path_default() {
        // Clear any env vars that might interfere
        // SAFETY: This is test code running in a single-threaded test context.
        // No other threads are concurrently accessing this environment variable.
        unsafe {
            std::env::remove_var(CORTEX_CONFIG_ENV);
        }

        let cortex_home = PathBuf::from("/test/cortex");
        let path = get_config_path(&cortex_home);
        assert_eq!(path, cortex_home.join(CONFIG_FILE));
    }

    #[test]
    fn test_strip_json_comments_single_line() {
        let input = r#"{
            // This is a comment
            "model": "gpt-4o"
        }"#;
        let stripped = strip_json_comments(input);
        assert!(!stripped.contains("//"));
        assert!(stripped.contains("\"model\""));
    }

    #[test]
    fn test_strip_json_comments_multi_line() {
        let input = r#"{
            /* This is a
               multi-line comment */
            "model": "gpt-4o"
        }"#;
        let stripped = strip_json_comments(input);
        assert!(!stripped.contains("/*"));
        assert!(!stripped.contains("*/"));
        assert!(stripped.contains("\"model\""));
    }

    #[test]
    fn test_strip_json_comments_preserves_strings() {
        let input = r#"{
            "url": "https://example.com/path",
            "comment": "This // is not a comment"
        }"#;
        let stripped = strip_json_comments(input);
        assert!(stripped.contains("https://example.com/path"));
        assert!(stripped.contains("This // is not a comment"));
    }

    #[test]
    fn test_config_format_detection() {
        assert_eq!(
            ConfigFormat::from_path(Path::new("config.toml")),
            ConfigFormat::Toml
        );
        assert_eq!(
            ConfigFormat::from_path(Path::new("config.json")),
            ConfigFormat::JsonC
        );
        assert_eq!(
            ConfigFormat::from_path(Path::new("config.jsonc")),
            ConfigFormat::JsonC
        );
        assert_eq!(
            ConfigFormat::from_path(Path::new("config")),
            ConfigFormat::Toml
        );
    }

    #[test]
    fn test_parse_json_config() {
        let json_content = r#"{
            // JSON with comments (JSONC) config
            "model": "claude-3-opus",
            "model_provider": "anthropic"
        }"#;

        let config = parse_config_content(json_content, ConfigFormat::JsonC).unwrap();
        assert_eq!(config.model, Some("claude-3-opus".to_string()));
        assert_eq!(config.model_provider, Some("anthropic".to_string()));
    }

    #[tokio::test]
    async fn test_load_json_config() {
        let temp_dir = TempDir::new().unwrap();
        let config_content = r#"{
            // This is a JSONC config file
            "model": "gpt-4o",
            "model_provider": "openai"
        }"#;
        tokio::fs::write(temp_dir.path().join("config.json"), config_content)
            .await
            .unwrap();

        let config = load_config_from_path(&temp_dir.path().join("config.json"))
            .await
            .unwrap();
        assert_eq!(config.model, Some("gpt-4o".to_string()));
        assert_eq!(config.model_provider, Some("openai".to_string()));
    }
}
