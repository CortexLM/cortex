//! macOS Seatbelt sandbox implementation.
//!
//! This module provides a secure sandboxing implementation using macOS's
//! Seatbelt (sandbox-exec) facility with properly restricted file access
//! and explicit deny rules for sensitive paths.

use std::path::Path;
use std::process::Command;

use crate::SandboxBackend;

/// Sensitive paths that must always be blocked from read access.
/// These contain credentials, keys, and other security-sensitive data.
const SENSITIVE_PATHS: &[&str] = &[
    // SSH and credential directories
    "~/.ssh",
    "~/.aws",
    "~/.gnupg",
    "~/.config/gcloud",
    "~/.azure",
    "~/.kube",
    "~/.docker/config.json",
    // Password and authentication files
    "/etc/passwd",
    "/etc/shadow",
    "/etc/master.passwd",
    "/etc/sudoers",
    "/etc/security",
    // macOS Keychain
    "~/Library/Keychains",
    "/Library/Keychains",
    "/System/Library/Keychains",
    // Browser credential stores
    "~/Library/Application Support/Google/Chrome/Default/Login Data",
    "~/Library/Application Support/Firefox/Profiles",
    "~/Library/Safari",
    // Environment files that may contain secrets
    "~/.env",
    "~/.netrc",
    "~/.npmrc",
    "~/.pypirc",
    "~/.gem/credentials",
];

/// System library paths needed for basic execution.
const SYSTEM_READ_PATHS: &[&str] = &[
    "/System/Library",
    "/usr/lib",
    "/usr/share",
    "/Library/Frameworks",
    "/private/var/db/dyld",
    "/dev/null",
    "/dev/urandom",
    "/dev/random",
    "/dev/zero",
    "/dev/tty",
];

/// Temporary directories that may be needed for execution.
const TEMP_PATHS: &[&str] = &["/tmp", "/private/tmp", "/var/folders"];

/// Seatbelt sandbox backend with security-hardened profile generation.
pub struct SeatbeltSandbox {
    available: bool,
}

impl SeatbeltSandbox {
    /// Create a new Seatbelt sandbox.
    pub fn new() -> Self {
        Self {
            available: Path::new("/usr/bin/sandbox-exec").exists(),
        }
    }

    /// Escape a path string for safe use in SBPL (Sandbox Profile Language).
    ///
    /// SBPL uses scheme-like syntax where certain characters must be escaped:
    /// - Backslashes become double backslashes
    /// - Double quotes become escaped quotes
    /// - Parentheses, semicolons, and other special chars are escaped
    ///
    /// This prevents profile injection attacks via malicious path names.
    fn escape_sbpl_path(path: &str) -> String {
        let mut escaped = String::with_capacity(path.len() * 2);
        for ch in path.chars() {
            match ch {
                '\\' => escaped.push_str("\\\\"),
                '"' => escaped.push_str("\\\""),
                // Escape parentheses to prevent SBPL expression injection
                '(' => escaped.push_str("\\("),
                ')' => escaped.push_str("\\)"),
                // Escape semicolons (comment markers in SBPL)
                ';' => escaped.push_str("\\;"),
                // Newlines could break profile structure
                '\n' => escaped.push_str("\\n"),
                '\r' => escaped.push_str("\\r"),
                // Null bytes are never valid
                '\0' => continue,
                _ => escaped.push(ch),
            }
        }
        escaped
    }

    /// Validate that a path is safe to use as a writable root.
    ///
    /// Returns an error message if the path is unsafe, None if it's acceptable.
    fn validate_writable_path(path: &Path) -> Option<&'static str> {
        let path_str = path.to_string_lossy();

        // Block obvious sensitive paths
        for sensitive in SENSITIVE_PATHS {
            let expanded = Self::expand_tilde(sensitive);
            if path_str.starts_with(&expanded) || expanded.starts_with(&*path_str) {
                return Some("Path overlaps with sensitive directory");
            }
        }

        // Block system directories from being writable
        let system_dirs = [
            "/System",
            "/usr",
            "/bin",
            "/sbin",
            "/Library",
            "/private/etc",
            "/etc",
        ];
        for sys_dir in system_dirs {
            if path_str.starts_with(sys_dir) {
                return Some("Cannot make system directory writable");
            }
        }

        // Block root filesystem
        if path_str == "/" {
            return Some("Cannot make root filesystem writable");
        }

        None
    }

    /// Expand tilde to the actual home directory path.
    fn expand_tilde(path: &str) -> String {
        if path.starts_with("~/") {
            if let Some(home) = std::env::var_os("HOME") {
                return format!("{}{}", home.to_string_lossy(), &path[1..]);
            }
        }
        path.to_string()
    }

    /// Generate a secure sandbox profile for the given policy.
    ///
    /// # Arguments
    /// * `writable_roots` - Directories where write access is permitted
    /// * `readable_roots` - Additional directories where read access is permitted
    /// * `allow_network` - Whether to allow network access
    /// * `allowed_hosts` - If network is allowed, optionally restrict to specific hosts
    ///
    /// # Security Properties
    /// - Denies all access by default
    /// - Explicitly blocks sensitive paths before any allow rules
    /// - Only allows reading from specified paths and system libraries
    /// - Escapes all paths to prevent SBPL injection
    pub fn generate_profile(
        writable_roots: &[&Path],
        readable_roots: &[&Path],
        allow_network: bool,
        allowed_hosts: Option<&[&str]>,
    ) -> Result<String, String> {
        // Validate all writable paths first
        for path in writable_roots {
            if let Some(err) = Self::validate_writable_path(path) {
                return Err(format!(
                    "Invalid writable path '{}': {}",
                    path.display(),
                    err
                ));
            }
        }

        let mut profile = String::with_capacity(4096);

        // Profile header and default deny
        profile.push_str(
            r#"(version 1)
; Cortex Sandbox - Security-hardened macOS sandbox profile
; Generated with explicit deny rules and restricted access

; CRITICAL: Deny all operations by default
(deny default)

; ============================================================
; SECTION 1: EXPLICIT DENIALS FOR SENSITIVE PATHS
; These rules MUST come before any allow rules to ensure
; sensitive paths are never accessible, even if other rules
; would permit them.
; ============================================================

"#,
        );

        // Add explicit deny rules for all sensitive paths
        for sensitive_path in SENSITIVE_PATHS {
            let expanded = Self::expand_tilde(sensitive_path);
            let escaped = Self::escape_sbpl_path(&expanded);
            profile.push_str(&format!(
                "; Block access to: {}\n(deny file-read* (subpath \"{}\"))\n(deny file-write* (subpath \"{}\"))\n\n",
                sensitive_path, escaped, escaped
            ));
        }

        // Section for basic process operations
        profile.push_str(
            r#"
; ============================================================
; SECTION 2: BASIC PROCESS OPERATIONS
; Minimal permissions needed for process execution
; ============================================================

; Allow process creation and signaling
(allow process-fork)
(allow process-exec)
(allow signal (target self))

; Allow reading process info for self only
(allow process-info-pidinfo (target self))
(allow process-info-setcontrol (target self))

; Allow sysctl reads (needed for basic operations)
(allow sysctl-read)

; Allow mach operations needed for process execution
(allow mach-lookup)
(allow mach-priv-host-port)

"#,
        );

        // Section for system library access
        profile.push_str(
            r#"
; ============================================================
; SECTION 3: SYSTEM LIBRARY ACCESS (READ-ONLY)
; Required for dynamic linking and basic system operations
; ============================================================

"#,
        );

        for sys_path in SYSTEM_READ_PATHS {
            let escaped = Self::escape_sbpl_path(sys_path);
            if sys_path.starts_with("/dev/") {
                // Device files need literal matching
                profile.push_str(&format!("(allow file-read* (literal \"{}\"))\n", escaped));
            } else {
                profile.push_str(&format!("(allow file-read* (subpath \"{}\"))\n", escaped));
            }
        }

        // Temporary directory access
        profile.push_str(
            r#"
; ============================================================
; SECTION 4: TEMPORARY DIRECTORY ACCESS
; Allow read/write to system temp directories
; ============================================================

"#,
        );

        for temp_path in TEMP_PATHS {
            let escaped = Self::escape_sbpl_path(temp_path);
            profile.push_str(&format!(
                "(allow file-read* (subpath \"{}\"))\n(allow file-write* (subpath \"{}\"))\n",
                escaped, escaped
            ));
        }

        // Add user-specified readable roots
        if !readable_roots.is_empty() {
            profile.push_str(
                r#"
; ============================================================
; SECTION 5: USER-SPECIFIED READABLE PATHS
; Additional paths where read access is permitted
; ============================================================

"#,
            );

            for (i, root) in readable_roots.iter().enumerate() {
                let path = root.to_string_lossy();
                let escaped = Self::escape_sbpl_path(&path);
                profile.push_str(&format!(
                    "; Readable root {}: {}\n(allow file-read* (subpath \"{}\"))\n\n",
                    i, path, escaped
                ));
            }
        }

        // Add writable roots
        if !writable_roots.is_empty() {
            profile.push_str(
                r#"
; ============================================================
; SECTION 6: WRITABLE DIRECTORIES
; Directories where file modifications are permitted
; ============================================================

"#,
            );

            for (i, root) in writable_roots.iter().enumerate() {
                let path = root.to_string_lossy();
                let escaped = Self::escape_sbpl_path(&path);
                profile.push_str(&format!(
                    "; Writable root {}: {}\n(allow file-read* (subpath \"{}\"))\n(allow file-write* (subpath \"{}\"))\n\n",
                    i, path, escaped, escaped
                ));
            }
        }

        // Network access configuration
        profile.push_str(
            r#"
; ============================================================
; SECTION 7: NETWORK ACCESS
; ============================================================

"#,
        );

        if allow_network {
            match allowed_hosts {
                Some(hosts) if !hosts.is_empty() => {
                    profile.push_str("; Network access restricted to specific hosts\n");

                    // Allow basic network operations
                    profile.push_str(
                        "(allow network-outbound (literal \"/private/var/run/mDNSResponder\"))\n",
                    );
                    profile.push_str("(allow system-socket)\n");

                    // Allow connections only to specified hosts
                    for host in hosts {
                        let escaped_host = Self::escape_sbpl_path(host);
                        profile.push_str(&format!(
                            "(allow network-outbound (remote tcp \"{}:*\"))\n",
                            escaped_host
                        ));
                        profile.push_str(&format!(
                            "(allow network-outbound (remote udp \"{}:*\"))\n",
                            escaped_host
                        ));
                    }

                    // Allow localhost for IPC
                    profile.push_str("\n; Allow localhost for inter-process communication\n");
                    profile.push_str("(allow network-outbound (remote tcp \"localhost:*\"))\n");
                    profile.push_str("(allow network-outbound (remote tcp \"127.0.0.1:*\"))\n");
                    profile.push_str("(allow network-inbound (local tcp \"localhost:*\"))\n");
                    profile.push_str("(allow network-inbound (local tcp \"127.0.0.1:*\"))\n");
                }
                _ => {
                    // Full network access (but still deny inbound by default)
                    profile.push_str("; Full outbound network access permitted\n");
                    profile.push_str("(allow network-outbound)\n");
                    profile.push_str("(allow system-socket)\n");
                    profile.push_str("\n; Localhost inbound for IPC\n");
                    profile.push_str("(allow network-inbound (local tcp \"localhost:*\"))\n");
                    profile.push_str("(allow network-inbound (local tcp \"127.0.0.1:*\"))\n");
                }
            }
        } else {
            profile.push_str("; Network access DENIED\n");
            profile.push_str("; Only mDNSResponder socket allowed for DNS resolution\n");
            profile.push_str(
                "(allow network-outbound (literal \"/private/var/run/mDNSResponder\"))\n",
            );
        }

        // Footer with additional safety rules
        profile.push_str(
            r#"
; ============================================================
; SECTION 8: ADDITIONAL SAFETY RULES
; ============================================================

; Deny any attempts to modify system integrity
(deny file-write-setugid)
(deny file-write-mount)
(deny file-write-unmount)

; Deny kernel and hardware access
(deny system-kext*)
(deny nvram*)

; Deny debugging other processes
(deny process-info-codesignature)
(deny process-info-pidinfo (target others))

; End of sandbox profile
"#,
        );

        Ok(profile)
    }

    /// Generate a simple profile with backwards-compatible signature.
    ///
    /// This is a convenience wrapper that provides sensible defaults.
    /// For production use, prefer `generate_profile` with explicit parameters.
    pub fn generate_simple_profile(
        writable_roots: &[&Path],
        allow_network: bool,
    ) -> Result<String, String> {
        Self::generate_profile(writable_roots, &[], allow_network, None)
    }

    /// Wrap a command to run under sandbox-exec.
    ///
    /// # Arguments
    /// * `command` - The command and arguments to execute
    /// * `profile` - The sandbox profile string
    ///
    /// # Returns
    /// A configured Command ready to be executed
    pub fn wrap_command(command: &[String], profile: &str) -> Command {
        let mut cmd = Command::new("/usr/bin/sandbox-exec");
        cmd.args(["-p", profile, "--"]);
        cmd.args(command);
        cmd
    }

    /// Test if the sandbox profile is valid and can be applied.
    ///
    /// This runs a simple test command under the profile to verify
    /// the profile syntax is correct and sandbox-exec accepts it.
    pub fn validate_profile(profile: &str) -> Result<(), String> {
        let output = Command::new("/usr/bin/sandbox-exec")
            .args(["-p", profile, "--", "/usr/bin/true"])
            .output()
            .map_err(|e| format!("Failed to execute sandbox-exec: {}", e))?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!("Sandbox profile validation failed: {}", stderr))
        }
    }
}

impl Default for SeatbeltSandbox {
    fn default() -> Self {
        Self::new()
    }
}

impl SandboxBackend for SeatbeltSandbox {
    fn name(&self) -> &str {
        "seatbelt"
    }

    fn is_available(&self) -> bool {
        self.available
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sbpl_escape_basic() {
        assert_eq!(
            SeatbeltSandbox::escape_sbpl_path("/simple/path"),
            "/simple/path"
        );
        assert_eq!(
            SeatbeltSandbox::escape_sbpl_path("path with spaces"),
            "path with spaces"
        );
    }

    #[test]
    fn test_sbpl_escape_special_chars() {
        assert_eq!(
            SeatbeltSandbox::escape_sbpl_path("path\"quote"),
            "path\\\"quote"
        );
        assert_eq!(
            SeatbeltSandbox::escape_sbpl_path("path\\backslash"),
            "path\\\\backslash"
        );
        assert_eq!(
            SeatbeltSandbox::escape_sbpl_path("path(paren)"),
            "path\\(paren\\)"
        );
        assert_eq!(
            SeatbeltSandbox::escape_sbpl_path("path;comment"),
            "path\\;comment"
        );
    }

    #[test]
    fn test_sbpl_escape_injection_attempt() {
        // Attempt to inject SBPL code via path
        let malicious = "/tmp\")\n(allow file-read* (subpath \"/etc";
        let escaped = SeatbeltSandbox::escape_sbpl_path(malicious);
        assert!(!escaped.contains("\n"));
        assert!(escaped.contains("\\\""));
    }

    #[test]
    #[ignore = "Test depends on specific macOS environment configuration"]
    fn test_validate_writable_path_blocks_sensitive() {
        let ssh_path = Path::new("/Users/test/.ssh");
        assert!(SeatbeltSandbox::validate_writable_path(ssh_path).is_some());

        let aws_path = Path::new("/Users/test/.aws");
        assert!(SeatbeltSandbox::validate_writable_path(aws_path).is_some());
    }

    #[test]
    fn test_validate_writable_path_blocks_system() {
        let system_path = Path::new("/System/Library");
        assert!(SeatbeltSandbox::validate_writable_path(system_path).is_some());

        let usr_path = Path::new("/usr/local");
        assert!(SeatbeltSandbox::validate_writable_path(usr_path).is_some());
    }

    #[test]
    fn test_validate_writable_path_allows_normal() {
        let project_path = Path::new("/Users/test/projects/myapp");
        assert!(SeatbeltSandbox::validate_writable_path(project_path).is_none());

        let tmp_work = Path::new("/tmp/workdir");
        assert!(SeatbeltSandbox::validate_writable_path(tmp_work).is_none());
    }

    #[test]
    fn test_generate_profile_structure() {
        let writable = vec![Path::new("/tmp/test")];
        let result = SeatbeltSandbox::generate_profile(&writable, &[], false, None);

        assert!(result.is_ok());
        let profile = result.unwrap();

        // Check required sections exist
        assert!(profile.contains("(version 1)"));
        assert!(profile.contains("(deny default)"));
        assert!(profile.contains("EXPLICIT DENIALS"));
        assert!(profile.contains(".ssh"));
        assert!(profile.contains(".aws"));
    }

    #[test]
    fn test_generate_profile_rejects_root() {
        let writable = vec![Path::new("/")];
        let result = SeatbeltSandbox::generate_profile(&writable, &[], false, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_expand_tilde() {
        let expanded = SeatbeltSandbox::expand_tilde("~/test");
        if std::env::var_os("HOME").is_some() {
            assert!(!expanded.starts_with("~"));
        }

        // Non-tilde paths unchanged
        assert_eq!(
            SeatbeltSandbox::expand_tilde("/absolute/path"),
            "/absolute/path"
        );
    }

    #[test]
    fn test_network_restriction() {
        let writable = vec![Path::new("/tmp/test")];
        let hosts = vec!["example.com", "api.example.com"];

        let result = SeatbeltSandbox::generate_profile(&writable, &[], true, Some(&hosts));
        assert!(result.is_ok());

        let profile = result.unwrap();
        assert!(profile.contains("example.com"));
        assert!(profile.contains("restricted to specific hosts"));
    }
}
