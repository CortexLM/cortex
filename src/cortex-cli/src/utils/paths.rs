//! Path utilities for the Cortex CLI.
//!
//! Provides functions for path manipulation, validation, and security checks.

use std::path::{Path, PathBuf};

/// Get the cortex home directory.
///
/// Uses the following precedence:
/// 1. `CORTEX_HOME` environment variable if set
/// 2. `~/.cortex` as fallback
///
/// # Returns
/// The path to the cortex home directory.
pub fn get_cortex_home() -> PathBuf {
    cortex_common::get_cortex_home().unwrap_or_else(|| {
        dirs::home_dir()
            .map(|h| h.join(".cortex"))
            .unwrap_or_else(|| PathBuf::from(".cortex"))
    })
}

/// Expand tilde (`~`) in a path to the user's home directory.
///
/// # Arguments
/// * `path` - The path string to expand
///
/// # Returns
/// The expanded path, or the original if home directory cannot be determined.
///
/// # Example
/// ```ignore
/// let expanded = expand_tilde("~/documents/file.txt");
/// // Returns: /home/user/documents/file.txt
/// ```
pub fn expand_tilde(path: &str) -> String {
    if path.starts_with("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(&path[2..]).to_string_lossy().to_string();
    }
    path.to_string()
}

/// Validate a path for security concerns.
///
/// Checks for:
/// - Path traversal attempts (`..`)
/// - System-critical paths that should not be modified
/// - Device files on Unix systems
///
/// # Arguments
/// * `path` - The path to validate
/// * `base_dir` - Optional base directory for containment check
///
/// # Returns
/// `Ok(())` if the path is safe, or an error message describing the issue.
pub fn validate_path_safety(path: &Path, base_dir: Option<&Path>) -> Result<(), String> {
    let path_str = path.to_string_lossy();

    // Check for path traversal attempts
    if path_str.contains("..") {
        return Err("Path contains traversal sequence '..'".to_string());
    }

    // Check for absolute paths that escape the base directory
    if let Some(base) = base_dir
        && path.is_absolute()
    {
        let canonical_path = path.canonicalize().ok();
        let canonical_base = base.canonicalize().ok();

        if let (Some(cp), Some(cb)) = (canonical_path, canonical_base)
            && !cp.starts_with(&cb)
        {
            return Err(format!(
                "Path '{}' escapes base directory '{}'",
                path.display(),
                base.display()
            ));
        }
    }

    // Block system-critical paths
    let critical_paths = [
        "/etc", "/usr", "/bin", "/sbin", "/lib", "/lib64", "/boot", "/root", "/var/run",
        "/var/lib", "/proc", "/sys", "/dev",
    ];

    for critical in critical_paths {
        if path_str.starts_with(critical) && !path_str.starts_with("/etc/cortex") {
            return Err(format!(
                "Path '{}' is in a protected system directory",
                path.display()
            ));
        }
    }

    Ok(())
}

/// Sensitive system paths that should trigger a security warning when read.
pub const SENSITIVE_PATHS: &[&str] = &[
    "/etc/passwd",
    "/etc/shadow",
    "/etc/group",
    "/etc/gshadow",
    "/etc/sudoers",
    "/etc/ssh/",
    "/etc/ssl/",
    "/etc/pki/",
    "/.ssh/",
    "/.gnupg/",
    "/.aws/",
    "/.azure/",
    "/.gcloud/",
    "/.kube/",
    "/.docker/config.json",
    "/.npmrc",
    "/.pypirc",
    "/.netrc",
    "/.gitconfig",
    "/id_rsa",
    "/id_dsa",
    "/id_ecdsa",
    "/id_ed25519",
    "/.env",
    "/credentials",
    "/secrets",
    "/private",
    "/token",
];

/// Check if a path appears to be a sensitive system file.
///
/// # Arguments
/// * `path` - The path to check
///
/// # Returns
/// `true` if the path appears sensitive, `false` otherwise.
pub fn is_sensitive_path(path: &Path) -> bool {
    let path_str = path.to_string_lossy().to_lowercase();

    for sensitive in SENSITIVE_PATHS {
        if path_str.contains(&sensitive.to_lowercase()) {
            return true;
        }
    }

    // Also check for common sensitive file extensions
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext_lower = ext.to_lowercase();
        if matches!(
            ext_lower.as_str(),
            "pem" | "key" | "crt" | "cer" | "pfx" | "p12"
        ) {
            return true;
        }
    }

    false
}

/// Join path components safely, preventing path traversal.
///
/// # Arguments
/// * `base` - The base directory
/// * `path` - The path to join
///
/// # Returns
/// The joined path if safe, or an error if path traversal was detected.
pub fn safe_join(base: &Path, path: &str) -> Result<PathBuf, String> {
    // First expand tilde
    let expanded = expand_tilde(path);
    let joined = base.join(&expanded);

    // Validate the result
    validate_path_safety(&joined, Some(base))?;

    Ok(joined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_expand_tilde() {
        // Just test that it doesn't panic on empty or regular paths
        assert_eq!(expand_tilde(""), "");
        assert_eq!(expand_tilde("/absolute/path"), "/absolute/path");
        assert_eq!(expand_tilde("relative/path"), "relative/path");
        // Tilde expansion depends on HOME env var, so we just check it doesn't panic
        let _ = expand_tilde("~/some/path");
    }

    #[test]
    fn test_path_traversal_detection() {
        let path = Path::new("../../../etc/passwd");
        let result = validate_path_safety(path, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("traversal"));
    }

    #[test]
    fn test_sensitive_path_detection() {
        assert!(is_sensitive_path(Path::new("/home/user/.ssh/id_rsa")));
        assert!(is_sensitive_path(Path::new("/home/user/.aws/credentials")));
        assert!(is_sensitive_path(Path::new("secret.pem")));
        assert!(!is_sensitive_path(Path::new(
            "/home/user/documents/file.txt"
        )));
    }

    // ==================== Additional tests ====================

    #[test]
    fn test_get_cortex_home_returns_path() {
        // Ensure get_cortex_home returns a valid path that either exists or can be created
        let home = get_cortex_home();
        // The path should contain "cortex" - either from env var or default
        let path_str = home.to_string_lossy();
        // Just verify it returns a path containing "cortex"
        assert!(
            path_str.contains("cortex"),
            "Expected cortex home to contain 'cortex', got: {}",
            path_str
        );
    }

    #[test]
    fn test_get_cortex_home_with_env_variable() {
        // Save original value
        let original = env::var("CORTEX_HOME").ok();

        // Set custom CORTEX_HOME
        // SAFETY: This test runs in a single-threaded context and we restore the value afterwards
        unsafe {
            env::set_var("CORTEX_HOME", "/tmp/custom_cortex_home");
        }
        // Note: cortex_common::get_cortex_home() might cache the value,
        // but we test that our function doesn't panic
        let home = get_cortex_home();
        // The function should return a valid PathBuf
        assert!(!home.as_os_str().is_empty());

        // Restore original
        // SAFETY: This test runs in a single-threaded context
        unsafe {
            match original {
                Some(val) => env::set_var("CORTEX_HOME", val),
                None => env::remove_var("CORTEX_HOME"),
            }
        }
    }

    #[test]
    fn test_expand_tilde_with_tilde_only() {
        // Test tilde alone - should remain unchanged (not "~/")
        assert_eq!(expand_tilde("~"), "~");
    }

    #[test]
    fn test_expand_tilde_with_tilde_in_middle() {
        // Tilde in the middle should NOT be expanded
        assert_eq!(expand_tilde("/path/~/file"), "/path/~/file");
    }

    #[test]
    fn test_expand_tilde_with_tilde_prefix_expands() {
        // Test that ~/ prefix triggers expansion (when home dir is available)
        let result = expand_tilde("~/test/file.txt");
        // If home directory is available, the path should be expanded
        if let Some(home) = dirs::home_dir() {
            let expected = home.join("test/file.txt").to_string_lossy().to_string();
            assert_eq!(result, expected);
        } else {
            // If no home dir, original is returned
            assert_eq!(result, "~/test/file.txt");
        }
    }

    #[test]
    fn test_validate_path_safety_allows_safe_paths() {
        // Safe relative path
        let path = Path::new("safe/path/to/file.txt");
        assert!(validate_path_safety(path, None).is_ok());

        // Safe absolute path outside critical directories
        let path = Path::new("/home/user/documents/file.txt");
        assert!(validate_path_safety(path, None).is_ok());
    }

    #[test]
    fn test_validate_path_safety_blocks_critical_paths() {
        // /etc is critical
        let path = Path::new("/etc/hosts");
        let result = validate_path_safety(path, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("protected system directory"));

        // /usr is critical
        let path = Path::new("/usr/bin/something");
        let result = validate_path_safety(path, None);
        assert!(result.is_err());

        // /dev is critical
        let path = Path::new("/dev/null");
        let result = validate_path_safety(path, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_path_safety_allows_etc_cortex() {
        // /etc/cortex is special-cased to be allowed
        let path = Path::new("/etc/cortex/config.toml");
        assert!(validate_path_safety(path, None).is_ok());
    }

    #[test]
    fn test_validate_path_safety_detects_various_traversal_patterns() {
        // Different traversal patterns
        let patterns = ["foo/../bar", "...", "foo/bar/../baz", "./foo/../../../etc"];

        for pattern in patterns {
            let path = Path::new(pattern);
            let result = validate_path_safety(path, None);
            // Only patterns containing ".." should fail
            if pattern.contains("..") {
                assert!(
                    result.is_err(),
                    "Expected traversal detection for: {}",
                    pattern
                );
            }
        }
    }

    #[test]
    fn test_is_sensitive_path_detects_ssh_keys() {
        assert!(is_sensitive_path(Path::new("/home/user/.ssh/id_rsa")));
        assert!(is_sensitive_path(Path::new("/home/user/.ssh/id_dsa")));
        assert!(is_sensitive_path(Path::new("/home/user/.ssh/id_ecdsa")));
        assert!(is_sensitive_path(Path::new("/home/user/.ssh/id_ed25519")));
    }

    #[test]
    fn test_is_sensitive_path_detects_cloud_credentials() {
        assert!(is_sensitive_path(Path::new("/home/user/.aws/credentials")));
        assert!(is_sensitive_path(Path::new("/home/user/.azure/config")));
        assert!(is_sensitive_path(Path::new(
            "/home/user/.gcloud/credentials"
        )));
        assert!(is_sensitive_path(Path::new("/home/user/.kube/config")));
    }

    #[test]
    fn test_is_sensitive_path_detects_env_files() {
        assert!(is_sensitive_path(Path::new("/project/.env")));
        assert!(is_sensitive_path(Path::new("/project/.env.local")));
    }

    #[test]
    fn test_is_sensitive_path_detects_certificate_extensions() {
        assert!(is_sensitive_path(Path::new("server.pem")));
        assert!(is_sensitive_path(Path::new("private.key")));
        assert!(is_sensitive_path(Path::new("certificate.crt")));
        assert!(is_sensitive_path(Path::new("ca.cer")));
        assert!(is_sensitive_path(Path::new("keystore.pfx")));
        assert!(is_sensitive_path(Path::new("keystore.p12")));
    }

    #[test]
    fn test_is_sensitive_path_case_insensitive() {
        // Extensions should be case-insensitive
        assert!(is_sensitive_path(Path::new("cert.PEM")));
        assert!(is_sensitive_path(Path::new("cert.Key")));
        assert!(is_sensitive_path(Path::new("cert.CRT")));
    }

    #[test]
    fn test_is_sensitive_path_returns_false_for_safe_files() {
        assert!(!is_sensitive_path(Path::new(
            "/home/user/documents/report.pdf"
        )));
        assert!(!is_sensitive_path(Path::new("/home/user/code/main.rs")));
        assert!(!is_sensitive_path(Path::new("/tmp/data.json")));
        assert!(!is_sensitive_path(Path::new("README.md")));
    }

    #[test]
    fn test_safe_join_with_safe_path() {
        let base = Path::new("/home/user/project");
        let result = safe_join(base, "src/main.rs");
        // safe_join should succeed for safe paths within base
        // Note: actual success depends on whether the paths exist for canonicalize
        // If paths don't exist, we just verify it doesn't fail on traversal check
        match result {
            Ok(joined) => {
                assert!(joined.to_string_lossy().contains("src/main.rs"));
            }
            Err(e) => {
                // If it fails, it should not be due to traversal
                assert!(
                    !e.contains("traversal"),
                    "Unexpected traversal error for safe path"
                );
            }
        }
    }

    #[test]
    fn test_safe_join_rejects_traversal() {
        let base = Path::new("/home/user/project");
        let result = safe_join(base, "../../../etc/passwd");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("traversal"));
    }

    #[test]
    fn test_safe_join_expands_tilde() {
        let base = Path::new("/home/user/project");
        // If tilde is used, it gets expanded before joining
        let result = safe_join(base, "~/other/file");
        // The result should contain the expanded home directory
        // But this might fail validation if it escapes base_dir
        // The important thing is tilde is expanded
        match result {
            Ok(path) => {
                // If it succeeds, tilde should be expanded
                assert!(
                    !path.to_string_lossy().contains("~"),
                    "Tilde should be expanded"
                );
            }
            Err(_) => {
                // Might fail because expanded path escapes base dir
                // That's valid behavior
            }
        }
    }

    #[test]
    fn test_safe_join_with_empty_path() {
        let base = Path::new("/home/user/project");
        let result = safe_join(base, "");
        // Empty path joined with base should just be the base
        match result {
            Ok(path) => {
                assert_eq!(path, base.join(""));
            }
            Err(e) => {
                // Should not fail with traversal error
                assert!(!e.contains("traversal"));
            }
        }
    }

    #[test]
    fn test_sensitive_paths_constant_includes_expected_entries() {
        // Verify the SENSITIVE_PATHS constant contains expected sensitive locations
        assert!(SENSITIVE_PATHS.contains(&"/etc/passwd"));
        assert!(SENSITIVE_PATHS.contains(&"/etc/shadow"));
        assert!(SENSITIVE_PATHS.contains(&"/.ssh/"));
        assert!(SENSITIVE_PATHS.contains(&"/.aws/"));
        assert!(SENSITIVE_PATHS.contains(&"/.env"));
        assert!(SENSITIVE_PATHS.contains(&"/credentials"));
    }

    #[test]
    fn test_is_sensitive_path_detects_secrets_and_private() {
        assert!(is_sensitive_path(Path::new("/app/secrets/api_key")));
        assert!(is_sensitive_path(Path::new("/data/private/config")));
        assert!(is_sensitive_path(Path::new("/home/user/token.txt")));
    }

    #[test]
    fn test_is_sensitive_path_detects_docker_config() {
        assert!(is_sensitive_path(Path::new(
            "/home/user/.docker/config.json"
        )));
    }

    #[test]
    fn test_is_sensitive_path_detects_npm_pypi_config() {
        assert!(is_sensitive_path(Path::new("/home/user/.npmrc")));
        assert!(is_sensitive_path(Path::new("/home/user/.pypirc")));
        assert!(is_sensitive_path(Path::new("/home/user/.netrc")));
    }

    #[test]
    fn test_validate_path_safety_blocks_all_critical_dirs() {
        let critical_paths = [
            "/etc/hosts",
            "/usr/local/bin/tool",
            "/bin/bash",
            "/sbin/init",
            "/lib/libfoo.so",
            "/lib64/libbar.so",
            "/boot/vmlinuz",
            "/root/.bashrc",
            "/var/run/pid",
            "/var/lib/data",
            "/proc/1/status",
            "/sys/class/net",
            "/dev/sda",
        ];

        for path_str in critical_paths {
            let path = Path::new(path_str);
            let result = validate_path_safety(path, None);
            assert!(
                result.is_err(),
                "Expected '{}' to be blocked as critical path",
                path_str
            );
        }
    }
}
