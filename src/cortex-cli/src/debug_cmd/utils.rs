//! Shared utility functions for debug commands.

use anyhow::Result;
use std::path::PathBuf;

// =============================================================================
// Path utilities
// =============================================================================

/// Get the cortex home directory.
pub fn get_cortex_home() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".cortex"))
        .unwrap_or_else(|| PathBuf::from(".cortex"))
}

/// Get the cortex home directory (alternative function name for clarity).
pub fn get_cortex_home_or_default() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".cortex"))
        .unwrap_or_else(|| PathBuf::from(".cortex"))
}

/// Calculate directory size.
pub fn dir_size(path: &PathBuf) -> Result<u64> {
    let mut total = 0;
    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let meta = entry.metadata()?;
            if meta.is_dir() {
                total += dir_size(&entry.path())?;
            } else {
                total += meta.len();
            }
        }
    }
    Ok(total)
}

/// Get the directories in the PATH environment variable.
pub fn get_path_directories() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default()
}

// =============================================================================
// Formatting utilities
// =============================================================================

/// Format file size in human-readable format.
pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

// =============================================================================
// Sensitive data handling
// =============================================================================

/// Patterns that indicate a variable contains sensitive data.
const SENSITIVE_PATTERNS: &[&str] = &[
    "API_KEY",
    "SECRET",
    "TOKEN",
    "PASSWORD",
    "CREDENTIAL",
    "PRIVATE",
    "AUTH",
    "ACCESS_KEY",
    "BEARER",
    "SESSION",
];

/// Check if an environment variable name indicates sensitive data.
pub fn is_sensitive_var_name(name: &str) -> bool {
    let name_upper = name.to_uppercase();
    SENSITIVE_PATTERNS
        .iter()
        .any(|pattern| name_upper.contains(pattern))
}

/// Redact a sensitive value, showing only first and last few characters.
pub fn redact_sensitive_value(value: &str) -> String {
    if value.is_empty() {
        return "[EMPTY]".to_string();
    }
    if value.len() <= 8 {
        return "[REDACTED]".to_string();
    }
    // Show first 4 and last 4 characters
    format!("{}...{}", &value[..4], &value[value.len() - 4..])
}

// =============================================================================
// Command checking
// =============================================================================

/// Check if a command is installed and get its path/version.
pub async fn check_command_installed(command: &str) -> (bool, Option<PathBuf>, Option<String>) {
    #[cfg(windows)]
    let which_cmd = "where";
    #[cfg(not(windows))]
    let which_cmd = "which";

    let output = tokio::process::Command::new(which_cmd)
        .arg(command)
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let path_str = String::from_utf8_lossy(&out.stdout);
            let path = path_str.lines().next().map(|s| PathBuf::from(s.trim()));

            // Try to get version
            let version = tokio::process::Command::new(command)
                .arg("--version")
                .output()
                .await
                .ok()
                .and_then(|v| {
                    if v.status.success() {
                        let ver = String::from_utf8_lossy(&v.stdout);
                        ver.lines().next().map(|s| s.trim().to_string())
                    } else {
                        None
                    }
                });

            (true, path, version)
        }
        _ => (false, None, None),
    }
}

// =============================================================================
// MIME type detection
// =============================================================================

/// Guess MIME type from file extension.
pub fn guess_mime_type(ext: &str) -> String {
    match ext.to_lowercase().as_str() {
        // Text
        "txt" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "csv" => "text/csv",
        "xml" => "text/xml",
        // Code
        "rs" => "text/x-rust",
        "js" => "text/javascript",
        "ts" => "text/typescript",
        "jsx" => "text/jsx",
        "tsx" => "text/tsx",
        "py" => "text/x-python",
        "rb" => "text/x-ruby",
        "go" => "text/x-go",
        "java" => "text/x-java",
        "c" | "h" => "text/x-c",
        "cpp" | "hpp" | "cc" => "text/x-c++",
        "cs" => "text/x-csharp",
        "swift" => "text/x-swift",
        "kt" => "text/x-kotlin",
        "sh" | "bash" => "text/x-shellscript",
        "ps1" => "text/x-powershell",
        "sql" => "text/x-sql",
        // Config
        "json" => "application/json",
        "yaml" | "yml" => "text/yaml",
        "toml" => "text/toml",
        "ini" | "cfg" => "text/plain",
        // Images
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        // Documents
        "pdf" => "application/pdf",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        // Archives
        "zip" => "application/zip",
        "tar" => "application/x-tar",
        "gz" => "application/gzip",
        // Executables
        "exe" => "application/x-msdownload",
        "dll" => "application/x-msdownload",
        "so" => "application/x-sharedlib",
        "dylib" => "application/x-mach-binary",
        // Default
        _ => "application/octet-stream",
    }
    .to_string()
}

// =============================================================================
// File system utilities
// =============================================================================

/// Check if the current user can write to the file.
///
/// This function attempts to open the file for writing to determine
/// if the current user has write access. This is more accurate than
/// checking permission bits, as it accounts for ownership, group
/// membership, and ACLs.
pub fn is_writable_by_current_user(path: &std::path::Path) -> bool {
    std::fs::OpenOptions::new().write(true).open(path).is_ok()
}

/// Get Unix permission mode and format as string (e.g., "rw-r--r--").
/// Returns (permission_string, numeric_mode) tuple.
#[cfg(unix)]
pub fn get_unix_permissions(meta: &std::fs::Metadata) -> (Option<String>, Option<u32>) {
    use std::os::unix::fs::PermissionsExt;
    let mode = meta.permissions().mode();
    // Extract just the permission bits (last 9 bits)
    let perm_bits = mode & 0o777;

    let permission_string = format!(
        "{}{}{}{}{}{}{}{}{}",
        if perm_bits & 0o400 != 0 { 'r' } else { '-' },
        if perm_bits & 0o200 != 0 { 'w' } else { '-' },
        if perm_bits & 0o100 != 0 { 'x' } else { '-' },
        if perm_bits & 0o040 != 0 { 'r' } else { '-' },
        if perm_bits & 0o020 != 0 { 'w' } else { '-' },
        if perm_bits & 0o010 != 0 { 'x' } else { '-' },
        if perm_bits & 0o004 != 0 { 'r' } else { '-' },
        if perm_bits & 0o002 != 0 { 'w' } else { '-' },
        if perm_bits & 0o001 != 0 { 'x' } else { '-' },
    );

    (Some(permission_string), Some(perm_bits))
}

/// Get Unix permission mode and format as string.
/// On non-Unix systems, returns None.
#[cfg(not(unix))]
pub fn get_unix_permissions(_meta: &std::fs::Metadata) -> (Option<String>, Option<u32>) {
    (None, None)
}

/// Detect special file types (FIFO, socket, block device, char device).
/// Uses stat() to determine the file type WITHOUT opening/reading the file,
/// which would block indefinitely for FIFOs and sockets.
pub fn detect_special_file_type(path: &std::path::Path) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let file_type = meta.file_type();
            if file_type.is_fifo() {
                return Some("fifo".to_string());
            }
            if file_type.is_socket() {
                return Some("socket".to_string());
            }
            if file_type.is_block_device() {
                return Some("block_device".to_string());
            }
            if file_type.is_char_device() {
                return Some("char_device".to_string());
            }
        }
        None
    }

    #[cfg(not(unix))]
    {
        // On non-Unix systems, special file types are not common
        let _ = path;
        None
    }
}

/// Check if the path is on a virtual filesystem like procfs or sysfs.
/// These filesystems report size=0 in stat() for files that have actual content. (#2829)
#[cfg(target_os = "linux")]
pub fn is_virtual_filesystem(path: &std::path::Path) -> bool {
    let path_str = path.to_string_lossy();
    path_str.starts_with("/proc/")
        || path_str.starts_with("/sys/")
        || path_str.starts_with("/dev/")
        || path_str == "/proc"
        || path_str == "/sys"
        || path_str == "/dev"
}

/// Check if the path is on a virtual filesystem like procfs or sysfs.
/// On non-Linux systems, return false as these filesystems are Linux-specific.
#[cfg(not(target_os = "linux"))]
pub fn is_virtual_filesystem(_path: &std::path::Path) -> bool {
    false
}

/// Detect encoding and binary status.
pub fn detect_encoding_and_binary(path: &PathBuf) -> (Option<String>, Option<bool>) {
    // Read first 8KB to check for binary content
    let Ok(file) = std::fs::File::open(path) else {
        return (None, None);
    };

    use std::io::Read;
    let mut reader = std::io::BufReader::new(file);
    let mut buffer = [0u8; 8192];
    let Ok(bytes_read) = reader.read(&mut buffer) else {
        return (None, None);
    };

    let sample = &buffer[..bytes_read];

    // Check for null bytes (common in binary files)
    let has_null = sample.contains(&0);

    // Check for UTF-8 BOM
    let has_utf8_bom = sample.starts_with(&[0xEF, 0xBB, 0xBF]);

    // Check for UTF-16 BOM
    let has_utf16_le_bom = sample.starts_with(&[0xFF, 0xFE]);
    let has_utf16_be_bom = sample.starts_with(&[0xFE, 0xFF]);

    let encoding = if has_utf8_bom {
        Some("UTF-8 (with BOM)".to_string())
    } else if has_utf16_le_bom {
        Some("UTF-16 LE".to_string())
    } else if has_utf16_be_bom {
        Some("UTF-16 BE".to_string())
    } else if !has_null && std::str::from_utf8(sample).is_ok() {
        Some("UTF-8".to_string())
    } else if has_null {
        Some("Binary".to_string())
    } else {
        Some("Unknown".to_string())
    };

    let is_binary = Some(has_null);

    (encoding, is_binary)
}

// =============================================================================
// System information utilities
// =============================================================================

/// Get OS version using platform-specific commands.
pub async fn get_os_version() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        // Try to get Windows version
        let output = tokio::process::Command::new("cmd")
            .args(["/c", "ver"])
            .output()
            .await
            .ok()?;
        if output.status.success() {
            let ver = String::from_utf8_lossy(&output.stdout);
            return Some(ver.trim().to_string());
        }
        None
    }

    #[cfg(target_os = "macos")]
    {
        // Try to get macOS version
        let output = tokio::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .await
            .ok()?;
        if output.status.success() {
            let ver = String::from_utf8_lossy(&output.stdout);
            return Some(ver.trim().to_string());
        }
        None
    }

    #[cfg(target_os = "linux")]
    {
        // Try to read /etc/os-release
        if let Ok(content) = std::fs::read_to_string("/etc/os-release") {
            for line in content.lines() {
                if line.starts_with("PRETTY_NAME=") {
                    let name = line
                        .trim_start_matches("PRETTY_NAME=")
                        .trim_matches('"')
                        .to_string();
                    return Some(name);
                }
            }
        }
        // Fallback to uname
        let output = tokio::process::Command::new("uname")
            .arg("-r")
            .output()
            .await
            .ok()?;
        if output.status.success() {
            let ver = String::from_utf8_lossy(&output.stdout);
            return Some(ver.trim().to_string());
        }
        None
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

/// Get Rust version if rustc is available.
pub async fn get_rust_version() -> Option<String> {
    let output = tokio::process::Command::new("rustc")
        .arg("--version")
        .output()
        .await
        .ok()?;
    if output.status.success() {
        let ver = String::from_utf8_lossy(&output.stdout);
        return Some(ver.trim().to_string());
    }
    None
}

/// Get user information, handling container environments where UID may not be in /etc/passwd.
///
/// Returns (username, uid) where username falls back to "uid:<number>" if not found.
pub fn get_user_info() -> (Option<String>, Option<u32>) {
    // Try to get username from environment first
    let env_user = std::env::var("USER")
        .ok()
        .or_else(|| std::env::var("USERNAME").ok());

    #[cfg(unix)]
    {
        let uid = unsafe { libc::getuid() };

        // If we got a username from env, use it
        if let Some(user) = env_user {
            return (Some(user), Some(uid));
        }

        // Fallback: use UID string (common in containers with arbitrary UIDs)
        (Some(format!("uid:{}", uid)), Some(uid))
    }

    #[cfg(not(unix))]
    {
        (env_user, None)
    }
}

/// Get available memory, considering container cgroup limits on Linux.
///
/// Returns (memory_bytes, is_container_limited).
pub fn get_available_memory() -> (Option<u64>, Option<bool>) {
    #[cfg(target_os = "linux")]
    {
        // Try cgroup v2 first (unified hierarchy)
        if let Ok(limit) = std::fs::read_to_string("/sys/fs/cgroup/memory.max") {
            let limit = limit.trim();
            if limit != "max"
                && let Ok(bytes) = limit.parse::<u64>()
            {
                return (Some(bytes), Some(true));
            }
        }

        // Try cgroup v1
        if let Ok(limit) = std::fs::read_to_string("/sys/fs/cgroup/memory/memory.limit_in_bytes") {
            let limit = limit.trim();
            // Check if it's not the max value (which means unlimited)
            if let Ok(bytes) = limit.parse::<u64>() {
                // 9223372036854771712 is typically "unlimited" in cgroup v1
                if bytes < 9223372036854771712 {
                    return (Some(bytes), Some(true));
                }
            }
        }

        // Fall back to /proc/meminfo for host memory
        if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
            for line in meminfo.lines() {
                if line.starts_with("MemTotal:") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 2
                        && let Ok(kb) = parts[1].parse::<u64>()
                    {
                        return (Some(kb * 1024), Some(false));
                    }
                }
            }
        }

        (None, None)
    }

    #[cfg(target_os = "macos")]
    {
        // Use sysctl on macOS
        let output = std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok();

        if let Some(output) = output {
            if output.status.success() {
                let mem_str = String::from_utf8_lossy(&output.stdout);
                if let Ok(bytes) = mem_str.trim().parse::<u64>() {
                    return (Some(bytes), Some(false));
                }
            }
        }

        (None, None)
    }

    #[cfg(target_os = "windows")]
    {
        // On Windows, we could use GlobalMemoryStatusEx but it requires windows-sys crate
        // For now, return None
        (None, None)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        (None, None)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Path utilities tests
    // =========================================================================

    #[test]
    fn test_get_cortex_home_returns_valid_path() {
        let home = get_cortex_home();
        // Should end with .cortex
        assert!(home.ends_with(".cortex"));
    }

    #[test]
    fn test_get_cortex_home_or_default_returns_valid_path() {
        let home = get_cortex_home_or_default();
        // Should end with .cortex
        assert!(home.ends_with(".cortex"));
    }

    #[test]
    fn test_get_cortex_home_functions_return_same_path() {
        let home1 = get_cortex_home();
        let home2 = get_cortex_home_or_default();
        assert_eq!(home1, home2);
    }

    #[test]
    fn test_get_path_directories_returns_vec() {
        // PATH should be set in most environments
        let dirs = get_path_directories();
        // Result is a vec (could be empty if PATH is not set)
        // If PATH is set, directories should exist
        if !dirs.is_empty() {
            // At least verify they're PathBufs
            for dir in &dirs {
                assert!(!dir.as_os_str().is_empty());
            }
        }
    }

    #[test]
    fn test_dir_size_nonexistent_dir() {
        let path = PathBuf::from("/nonexistent/path/that/does/not/exist");
        // Should return Ok(0) since the path doesn't exist (not a directory)
        let result = dir_size(&path);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    #[test]
    fn test_dir_size_empty_temp_dir() {
        let temp_dir = std::env::temp_dir().join(format!("test_dir_size_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

        let result = dir_size(&temp_dir);
        assert!(result.is_ok());
        // Empty directory should have size 0
        assert_eq!(result.unwrap(), 0);

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_dir_size_with_file() {
        let temp_dir =
            std::env::temp_dir().join(format!("test_dir_size_file_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

        // Create a file with known size
        let file_path = temp_dir.join("test_file.txt");
        let content = "Hello, World!"; // 13 bytes
        std::fs::write(&file_path, content).expect("Failed to write test file");

        let result = dir_size(&temp_dir);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 13);

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_dir_size_with_nested_dirs() {
        let temp_dir =
            std::env::temp_dir().join(format!("test_dir_size_nested_{}", std::process::id()));
        let nested_dir = temp_dir.join("subdir");
        std::fs::create_dir_all(&nested_dir).expect("Failed to create nested dirs");

        // Create files in both directories
        std::fs::write(temp_dir.join("file1.txt"), "abc").expect("Failed to write file1");
        std::fs::write(nested_dir.join("file2.txt"), "defgh").expect("Failed to write file2");

        let result = dir_size(&temp_dir);
        assert!(result.is_ok());
        // 3 bytes + 5 bytes = 8 bytes
        assert_eq!(result.unwrap(), 8);

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    // =========================================================================
    // Formatting utilities tests
    // =========================================================================

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(500), "500 B");
        assert_eq!(format_size(1024), "1.00 KB");
        assert_eq!(format_size(1048576), "1.00 MB");
        assert_eq!(format_size(1073741824), "1.00 GB");
    }

    #[test]
    fn test_format_size_zero() {
        assert_eq!(format_size(0), "0 B");
    }

    #[test]
    fn test_format_size_boundary_values() {
        // Just under 1 KB
        assert_eq!(format_size(1023), "1023 B");
        // Exactly 1 KB
        assert_eq!(format_size(1024), "1.00 KB");
        // Just under 1 MB
        assert_eq!(format_size(1048575), "1024.00 KB");
        // Exactly 1 MB
        assert_eq!(format_size(1048576), "1.00 MB");
        // Just under 1 GB
        assert_eq!(format_size(1073741823), "1024.00 MB");
        // Exactly 1 GB
        assert_eq!(format_size(1073741824), "1.00 GB");
    }

    #[test]
    fn test_format_size_large_values() {
        // 10 GB
        assert_eq!(format_size(10 * 1073741824), "10.00 GB");
        // 100 GB
        assert_eq!(format_size(100 * 1073741824), "100.00 GB");
    }

    #[test]
    fn test_format_size_fractional_values() {
        // 1.5 KB = 1536 bytes
        assert_eq!(format_size(1536), "1.50 KB");
        // 2.5 MB
        assert_eq!(format_size(2621440), "2.50 MB");
    }

    // =========================================================================
    // Sensitive data handling tests
    // =========================================================================

    #[test]
    fn test_is_sensitive_var_name() {
        // Should match sensitive patterns
        assert!(is_sensitive_var_name("OPENAI_API_KEY"));
        assert!(is_sensitive_var_name("DATABASE_PASSWORD"));
        assert!(is_sensitive_var_name("AWS_SECRET_ACCESS_KEY"));
        assert!(is_sensitive_var_name("AUTH_TOKEN"));
        assert!(is_sensitive_var_name("GITHUB_TOKEN"));
        assert!(is_sensitive_var_name("PRIVATE_KEY"));
        assert!(is_sensitive_var_name("CREDENTIAL_FILE"));
        assert!(is_sensitive_var_name("BEARER_TOKEN"));

        // Should not match non-sensitive patterns
        assert!(!is_sensitive_var_name("PATH"));
        assert!(!is_sensitive_var_name("HOME"));
        assert!(!is_sensitive_var_name("USER"));
        assert!(!is_sensitive_var_name("EDITOR"));
        assert!(!is_sensitive_var_name("SHELL"));
    }

    #[test]
    fn test_is_sensitive_var_name_case_insensitive() {
        // Should match regardless of case
        assert!(is_sensitive_var_name("api_key"));
        assert!(is_sensitive_var_name("Api_Key"));
        assert!(is_sensitive_var_name("API_KEY"));
        assert!(is_sensitive_var_name("password"));
        assert!(is_sensitive_var_name("PASSWORD"));
        assert!(is_sensitive_var_name("PaSsWoRd"));
    }

    #[test]
    fn test_is_sensitive_var_name_session_pattern() {
        assert!(is_sensitive_var_name("SESSION_ID"));
        assert!(is_sensitive_var_name("SESSION_TOKEN"));
        assert!(is_sensitive_var_name("MY_SESSION"));
    }

    #[test]
    fn test_is_sensitive_var_name_access_key_pattern() {
        assert!(is_sensitive_var_name("AWS_ACCESS_KEY_ID"));
        assert!(is_sensitive_var_name("ACCESS_KEY"));
        assert!(is_sensitive_var_name("MY_ACCESS_KEY"));
    }

    #[test]
    fn test_redact_sensitive_value() {
        // Empty value
        assert_eq!(redact_sensitive_value(""), "[EMPTY]");

        // Short value (8 or fewer chars)
        assert_eq!(redact_sensitive_value("short"), "[REDACTED]");
        assert_eq!(redact_sensitive_value("12345678"), "[REDACTED]");

        // Longer value shows first/last 4 chars
        // "sk-abc123xyz789" has 15 chars: first 4 = "sk-a", last 4 = "z789"
        assert_eq!(redact_sensitive_value("sk-abc123xyz789"), "sk-a...z789");
        assert_eq!(redact_sensitive_value("supersecretpassword"), "supe...word");
    }

    #[test]
    fn test_redact_sensitive_value_boundary() {
        // Exactly 8 chars should be redacted
        assert_eq!(redact_sensitive_value("12345678"), "[REDACTED]");
        // 9 chars should show first/last 4
        assert_eq!(redact_sensitive_value("123456789"), "1234...6789");
    }

    #[test]
    fn test_redact_sensitive_value_single_char() {
        assert_eq!(redact_sensitive_value("a"), "[REDACTED]");
    }

    #[test]
    fn test_redact_sensitive_value_special_chars() {
        // Test with special characters
        assert_eq!(redact_sensitive_value("!@#$%^&*()"), "!@#$...&*()");
    }

    // =========================================================================
    // MIME type detection tests
    // =========================================================================

    #[test]
    fn test_guess_mime_type() {
        assert_eq!(guess_mime_type("rs"), "text/x-rust");
        assert_eq!(guess_mime_type("json"), "application/json");
        assert_eq!(guess_mime_type("png"), "image/png");
        assert_eq!(guess_mime_type("unknown"), "application/octet-stream");
    }

    #[test]
    fn test_guess_mime_type_case_insensitive() {
        assert_eq!(guess_mime_type("RS"), "text/x-rust");
        assert_eq!(guess_mime_type("JSON"), "application/json");
        assert_eq!(guess_mime_type("PNG"), "image/png");
        assert_eq!(guess_mime_type("Md"), "text/markdown");
    }

    #[test]
    fn test_guess_mime_type_text_files() {
        assert_eq!(guess_mime_type("txt"), "text/plain");
        assert_eq!(guess_mime_type("md"), "text/markdown");
        assert_eq!(guess_mime_type("markdown"), "text/markdown");
        assert_eq!(guess_mime_type("html"), "text/html");
        assert_eq!(guess_mime_type("htm"), "text/html");
        assert_eq!(guess_mime_type("css"), "text/css");
        assert_eq!(guess_mime_type("csv"), "text/csv");
        assert_eq!(guess_mime_type("xml"), "text/xml");
    }

    #[test]
    fn test_guess_mime_type_code_files() {
        assert_eq!(guess_mime_type("js"), "text/javascript");
        assert_eq!(guess_mime_type("ts"), "text/typescript");
        assert_eq!(guess_mime_type("jsx"), "text/jsx");
        assert_eq!(guess_mime_type("tsx"), "text/tsx");
        assert_eq!(guess_mime_type("py"), "text/x-python");
        assert_eq!(guess_mime_type("rb"), "text/x-ruby");
        assert_eq!(guess_mime_type("go"), "text/x-go");
        assert_eq!(guess_mime_type("java"), "text/x-java");
        assert_eq!(guess_mime_type("c"), "text/x-c");
        assert_eq!(guess_mime_type("h"), "text/x-c");
        assert_eq!(guess_mime_type("cpp"), "text/x-c++");
        assert_eq!(guess_mime_type("hpp"), "text/x-c++");
        assert_eq!(guess_mime_type("cc"), "text/x-c++");
        assert_eq!(guess_mime_type("cs"), "text/x-csharp");
        assert_eq!(guess_mime_type("swift"), "text/x-swift");
        assert_eq!(guess_mime_type("kt"), "text/x-kotlin");
    }

    #[test]
    fn test_guess_mime_type_shell_scripts() {
        assert_eq!(guess_mime_type("sh"), "text/x-shellscript");
        assert_eq!(guess_mime_type("bash"), "text/x-shellscript");
        assert_eq!(guess_mime_type("ps1"), "text/x-powershell");
    }

    #[test]
    fn test_guess_mime_type_config_files() {
        assert_eq!(guess_mime_type("json"), "application/json");
        assert_eq!(guess_mime_type("yaml"), "text/yaml");
        assert_eq!(guess_mime_type("yml"), "text/yaml");
        assert_eq!(guess_mime_type("toml"), "text/toml");
        assert_eq!(guess_mime_type("ini"), "text/plain");
        assert_eq!(guess_mime_type("cfg"), "text/plain");
    }

    #[test]
    fn test_guess_mime_type_images() {
        assert_eq!(guess_mime_type("png"), "image/png");
        assert_eq!(guess_mime_type("jpg"), "image/jpeg");
        assert_eq!(guess_mime_type("jpeg"), "image/jpeg");
        assert_eq!(guess_mime_type("gif"), "image/gif");
        assert_eq!(guess_mime_type("svg"), "image/svg+xml");
        assert_eq!(guess_mime_type("webp"), "image/webp");
        assert_eq!(guess_mime_type("ico"), "image/x-icon");
    }

    #[test]
    fn test_guess_mime_type_documents() {
        assert_eq!(guess_mime_type("pdf"), "application/pdf");
        assert_eq!(guess_mime_type("doc"), "application/msword");
        assert_eq!(
            guess_mime_type("docx"),
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        );
    }

    #[test]
    fn test_guess_mime_type_archives() {
        assert_eq!(guess_mime_type("zip"), "application/zip");
        assert_eq!(guess_mime_type("tar"), "application/x-tar");
        assert_eq!(guess_mime_type("gz"), "application/gzip");
    }

    #[test]
    fn test_guess_mime_type_executables() {
        assert_eq!(guess_mime_type("exe"), "application/x-msdownload");
        assert_eq!(guess_mime_type("dll"), "application/x-msdownload");
        assert_eq!(guess_mime_type("so"), "application/x-sharedlib");
        assert_eq!(guess_mime_type("dylib"), "application/x-mach-binary");
    }

    #[test]
    fn test_guess_mime_type_sql() {
        assert_eq!(guess_mime_type("sql"), "text/x-sql");
    }

    #[test]
    fn test_guess_mime_type_empty_extension() {
        assert_eq!(guess_mime_type(""), "application/octet-stream");
    }

    // =========================================================================
    // File system utilities tests
    // =========================================================================

    #[test]
    fn test_is_writable_by_current_user() {
        // Test with a temp file we create (should be writable)
        let temp_file = std::env::temp_dir().join(format!("test_writable_{}", std::process::id()));
        std::fs::write(&temp_file, "test").expect("Failed to write temp file");
        assert!(is_writable_by_current_user(&temp_file));
        let _ = std::fs::remove_file(&temp_file);

        // Test with a nonexistent file (should not be writable/openable)
        let nonexistent = PathBuf::from("/nonexistent/path/file.txt");
        assert!(!is_writable_by_current_user(&nonexistent));
    }

    #[cfg(unix)]
    #[test]
    fn test_get_unix_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temp_file =
            std::env::temp_dir().join(format!("test_permissions_{}", std::process::id()));
        std::fs::write(&temp_file, "test").expect("Failed to write temp file");

        // Set specific permissions
        let mut perms = std::fs::metadata(&temp_file)
            .expect("Failed to get metadata")
            .permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&temp_file, perms).expect("Failed to set permissions");

        let meta = std::fs::metadata(&temp_file).expect("Failed to get metadata");
        let (perm_str, perm_mode) = get_unix_permissions(&meta);

        assert_eq!(perm_str, Some("rw-r--r--".to_string()));
        assert_eq!(perm_mode, Some(0o644));

        let _ = std::fs::remove_file(&temp_file);
    }

    #[cfg(unix)]
    #[test]
    fn test_get_unix_permissions_executable() {
        use std::os::unix::fs::PermissionsExt;

        let temp_file =
            std::env::temp_dir().join(format!("test_permissions_exec_{}", std::process::id()));
        std::fs::write(&temp_file, "test").expect("Failed to write temp file");

        // Set executable permissions
        let mut perms = std::fs::metadata(&temp_file)
            .expect("Failed to get metadata")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&temp_file, perms).expect("Failed to set permissions");

        let meta = std::fs::metadata(&temp_file).expect("Failed to get metadata");
        let (perm_str, perm_mode) = get_unix_permissions(&meta);

        assert_eq!(perm_str, Some("rwxr-xr-x".to_string()));
        assert_eq!(perm_mode, Some(0o755));

        let _ = std::fs::remove_file(&temp_file);
    }

    #[cfg(unix)]
    #[test]
    fn test_get_unix_permissions_no_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temp_file =
            std::env::temp_dir().join(format!("test_permissions_none_{}", std::process::id()));
        std::fs::write(&temp_file, "test").expect("Failed to write temp file");

        // Set no permissions
        let mut perms = std::fs::metadata(&temp_file)
            .expect("Failed to get metadata")
            .permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&temp_file, perms).expect("Failed to set permissions");

        let meta = std::fs::metadata(&temp_file).expect("Failed to get metadata");
        let (perm_str, perm_mode) = get_unix_permissions(&meta);

        assert_eq!(perm_str, Some("---------".to_string()));
        assert_eq!(perm_mode, Some(0o000));

        // Restore permissions to clean up
        let mut perms = std::fs::metadata(&temp_file)
            .expect("Failed to get metadata")
            .permissions();
        perms.set_mode(0o644);
        let _ = std::fs::set_permissions(&temp_file, perms);
        let _ = std::fs::remove_file(&temp_file);
    }

    #[test]
    fn test_detect_special_file_type_regular_file() {
        let temp_file =
            std::env::temp_dir().join(format!("test_special_file_{}", std::process::id()));
        std::fs::write(&temp_file, "test").expect("Failed to write temp file");

        let result = detect_special_file_type(&temp_file);
        assert!(result.is_none()); // Regular file should return None

        let _ = std::fs::remove_file(&temp_file);
    }

    #[test]
    fn test_detect_special_file_type_directory() {
        let temp_dir =
            std::env::temp_dir().join(format!("test_special_dir_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

        let result = detect_special_file_type(&temp_dir);
        assert!(result.is_none()); // Directory should return None

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_detect_special_file_type_nonexistent() {
        let nonexistent = PathBuf::from("/nonexistent/path/file.txt");
        let result = detect_special_file_type(&nonexistent);
        assert!(result.is_none()); // Nonexistent file should return None
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_is_virtual_filesystem_proc() {
        assert!(is_virtual_filesystem(std::path::Path::new("/proc")));
        assert!(is_virtual_filesystem(std::path::Path::new("/proc/")));
        assert!(is_virtual_filesystem(std::path::Path::new("/proc/self")));
        assert!(is_virtual_filesystem(std::path::Path::new(
            "/proc/self/status"
        )));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_is_virtual_filesystem_sys() {
        assert!(is_virtual_filesystem(std::path::Path::new("/sys")));
        assert!(is_virtual_filesystem(std::path::Path::new("/sys/")));
        assert!(is_virtual_filesystem(std::path::Path::new("/sys/class")));
        assert!(is_virtual_filesystem(std::path::Path::new(
            "/sys/class/net"
        )));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_is_virtual_filesystem_dev() {
        assert!(is_virtual_filesystem(std::path::Path::new("/dev")));
        assert!(is_virtual_filesystem(std::path::Path::new("/dev/")));
        assert!(is_virtual_filesystem(std::path::Path::new("/dev/null")));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_is_virtual_filesystem_regular_paths() {
        assert!(!is_virtual_filesystem(std::path::Path::new("/home")));
        assert!(!is_virtual_filesystem(std::path::Path::new("/tmp")));
        assert!(!is_virtual_filesystem(std::path::Path::new("/usr/bin")));
        assert!(!is_virtual_filesystem(std::path::Path::new(
            "/var/log/syslog"
        )));
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn test_is_virtual_filesystem_non_linux() {
        // On non-Linux systems, always returns false
        assert!(!is_virtual_filesystem(std::path::Path::new("/proc")));
        assert!(!is_virtual_filesystem(std::path::Path::new("/sys")));
        assert!(!is_virtual_filesystem(std::path::Path::new("/dev")));
    }

    // =========================================================================
    // Encoding detection tests
    // =========================================================================

    #[test]
    fn test_detect_encoding_and_binary_utf8_file() {
        let temp_file =
            std::env::temp_dir().join(format!("test_encoding_utf8_{}", std::process::id()));
        std::fs::write(&temp_file, "Hello, World! UTF-8 text").expect("Failed to write temp file");

        let (encoding, is_binary) = detect_encoding_and_binary(&temp_file);
        assert_eq!(encoding, Some("UTF-8".to_string()));
        assert_eq!(is_binary, Some(false));

        let _ = std::fs::remove_file(&temp_file);
    }

    #[test]
    fn test_detect_encoding_and_binary_with_null_bytes() {
        let temp_file =
            std::env::temp_dir().join(format!("test_encoding_binary_{}", std::process::id()));
        // Write binary content with null bytes
        std::fs::write(&temp_file, b"Hello\x00World").expect("Failed to write temp file");

        let (encoding, is_binary) = detect_encoding_and_binary(&temp_file);
        assert_eq!(encoding, Some("Binary".to_string()));
        assert_eq!(is_binary, Some(true));

        let _ = std::fs::remove_file(&temp_file);
    }

    #[test]
    fn test_detect_encoding_and_binary_utf8_bom() {
        let temp_file =
            std::env::temp_dir().join(format!("test_encoding_utf8_bom_{}", std::process::id()));
        // Write UTF-8 BOM followed by content
        let mut content = vec![0xEF, 0xBB, 0xBF]; // UTF-8 BOM
        content.extend_from_slice(b"Hello");
        std::fs::write(&temp_file, &content).expect("Failed to write temp file");

        let (encoding, is_binary) = detect_encoding_and_binary(&temp_file);
        assert_eq!(encoding, Some("UTF-8 (with BOM)".to_string()));
        assert_eq!(is_binary, Some(false));

        let _ = std::fs::remove_file(&temp_file);
    }

    #[test]
    fn test_detect_encoding_and_binary_utf16_le_bom() {
        let temp_file =
            std::env::temp_dir().join(format!("test_encoding_utf16_le_{}", std::process::id()));
        // Write UTF-16 LE BOM
        let content = vec![0xFF, 0xFE, b'H', 0, b'i', 0];
        std::fs::write(&temp_file, &content).expect("Failed to write temp file");

        let (encoding, _is_binary) = detect_encoding_and_binary(&temp_file);
        assert_eq!(encoding, Some("UTF-16 LE".to_string()));

        let _ = std::fs::remove_file(&temp_file);
    }

    #[test]
    fn test_detect_encoding_and_binary_utf16_be_bom() {
        let temp_file =
            std::env::temp_dir().join(format!("test_encoding_utf16_be_{}", std::process::id()));
        // Write UTF-16 BE BOM
        let content = vec![0xFE, 0xFF, 0, b'H', 0, b'i'];
        std::fs::write(&temp_file, &content).expect("Failed to write temp file");

        let (encoding, _is_binary) = detect_encoding_and_binary(&temp_file);
        assert_eq!(encoding, Some("UTF-16 BE".to_string()));

        let _ = std::fs::remove_file(&temp_file);
    }

    #[test]
    fn test_detect_encoding_and_binary_nonexistent_file() {
        let nonexistent = PathBuf::from("/nonexistent/path/file.txt");
        let (encoding, is_binary) = detect_encoding_and_binary(&nonexistent);
        assert!(encoding.is_none());
        assert!(is_binary.is_none());
    }

    #[test]
    fn test_detect_encoding_and_binary_empty_file() {
        let temp_file =
            std::env::temp_dir().join(format!("test_encoding_empty_{}", std::process::id()));
        std::fs::write(&temp_file, "").expect("Failed to write temp file");

        let (encoding, is_binary) = detect_encoding_and_binary(&temp_file);
        // Empty file should be valid UTF-8
        assert_eq!(encoding, Some("UTF-8".to_string()));
        assert_eq!(is_binary, Some(false));

        let _ = std::fs::remove_file(&temp_file);
    }

    // =========================================================================
    // System information utilities tests
    // =========================================================================

    #[test]
    fn test_get_user_info_returns_values() {
        let (username, uid) = get_user_info();
        // Username should always be available (either from env or as uid:N)
        assert!(username.is_some());
        #[cfg(unix)]
        {
            // UID should be available on Unix
            assert!(uid.is_some());
        }
    }

    #[test]
    fn test_get_user_info_username_not_empty() {
        let (username, _) = get_user_info();
        let name = username.expect("Username should be present");
        assert!(!name.is_empty());
    }

    #[test]
    fn test_get_available_memory_returns_option() {
        let (memory, is_container) = get_available_memory();
        // On Linux, we should get some memory value
        #[cfg(target_os = "linux")]
        {
            assert!(memory.is_some());
            assert!(is_container.is_some());
            // Memory should be a reasonable value (at least 1MB)
            if let Some(mem) = memory {
                assert!(mem >= 1024 * 1024);
            }
        }
        // On other platforms, it depends on implementation
        let _ = (memory, is_container);
    }
}
