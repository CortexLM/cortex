//! macOS Seatbelt sandbox backend.
//!
//! Uses sandbox-exec with dynamically generated SBPL policies to provide:
//! - Filesystem isolation with writable workspace
//! - .git/.cortex protection via require-not clauses
//! - Optional network blocking

use std::path::PathBuf;

use super::manager::{CORTEX_SANDBOX_ENV_VAR, CORTEX_SANDBOX_NETWORK_DISABLED_ENV_VAR};
use super::policy::{SandboxPolicyType, WritableRoot};
use super::runner::{SandboxBackend, SandboxedCommand};
use crate::error::Result;

/// Path to sandbox-exec binary.
const MACOS_SANDBOX_EXEC_PATH: &str = "/usr/bin/sandbox-exec";

/// Base Seatbelt policy with security-hardened defaults.
const BASE_POLICY: &str = r#"(version 1)
; Cortex Sandbox - macOS Seatbelt Policy
; Deny all by default
(deny default)

; Allow basic process operations
(allow process-fork)
(allow process-exec)
(allow signal)

; Allow mach IPC (required for most programs)
(allow mach-lookup)
(allow mach-register)
(allow ipc-posix-shm*)

; Allow reading file metadata
(allow file-read-metadata)

; Allow reading system libraries and frameworks
(allow file-read*
    (subpath "/usr")
    (subpath "/System")
    (subpath "/Library/Frameworks")
    (subpath "/private/var/db/dyld"))

; Allow reading /dev devices
(allow file-read*
    (subpath "/dev"))

; Allow writing to /dev/null and /dev/tty
(allow file-write*
    (literal "/dev/null")
    (literal "/dev/tty")
    (literal "/dev/zero"))

; Allow reading /etc for timezone etc
(allow file-read*
    (literal "/etc/localtime")
    (literal "/etc/resolv.conf")
    (literal "/etc/hosts"))

; Allow sysctl reads (needed for various tools)
(allow sysctl-read)

"#;

/// Network policy addition when network access is allowed.
const NETWORK_POLICY: &str = r#"
; Allow network access
(allow network*)
(allow system-socket)
"#;

/// Seatbelt sandbox backend.
pub struct SeatbeltBackend {
    sandbox_exec_path: PathBuf,
}

impl SeatbeltBackend {
    /// Create a new Seatbelt backend.
    pub fn new() -> Self {
        Self {
            sandbox_exec_path: PathBuf::from(MACOS_SANDBOX_EXEC_PATH),
        }
    }

    /// Generate SBPL policy for the given sandbox policy.
    fn generate_policy(
        &self,
        policy: &SandboxPolicyType,
        writable_roots: &[WritableRoot],
    ) -> (String, Vec<(String, String)>) {
        let mut sbpl = BASE_POLICY.to_string();
        let mut params: Vec<(String, String)> = Vec::new();

        if policy.has_full_disk_write_access() {
            // Full write access (dangerous)
            sbpl.push_str("(allow file-write* (regex #\"^/\"))\n");
        } else if policy.has_full_disk_read_access() {
            // Allow reading from anywhere
            sbpl.push_str("; Allow reading from anywhere\n");
            sbpl.push_str("(allow file-read*)\n\n");
        }

        // Add writable roots with read-only subpath exclusions
        if !policy.has_full_disk_write_access() {
            for (i, root) in writable_roots.iter().enumerate() {
                let root_param = format!("WRITABLE_ROOT_{}", i);

                // Canonicalize path if possible
                let canonical_root = root
                    .root
                    .canonicalize()
                    .unwrap_or_else(|_| root.root.clone());
                params.push((root_param.clone(), canonical_root.display().to_string()));

                if root.read_only_subpaths.is_empty() {
                    // Simple case: just allow writes to the root
                    sbpl.push_str(&format!(
                        "(allow file-write* (subpath (param \"{}\")))\n",
                        root_param
                    ));
                } else {
                    // Complex case: allow writes but exclude read-only subpaths
                    let mut require_parts = vec![format!("(subpath (param \"{}\"))", root_param)];

                    for (j, ro_path) in root.read_only_subpaths.iter().enumerate() {
                        let ro_param = format!("WRITABLE_ROOT_{}_RO_{}", i, j);
                        let canonical_ro =
                            ro_path.canonicalize().unwrap_or_else(|_| ro_path.clone());
                        params.push((ro_param.clone(), canonical_ro.display().to_string()));

                        require_parts
                            .push(format!("(require-not (subpath (param \"{}\")))", ro_param));
                    }

                    sbpl.push_str(&format!(
                        "(allow file-write* (require-all {}))\n",
                        require_parts.join(" ")
                    ));
                }
            }
        }

        // Add network policy if allowed
        if policy.has_full_network_access() {
            sbpl.push_str(NETWORK_POLICY);
        }

        // Add Darwin user cache dir if available
        if let Some(cache_dir) = get_darwin_cache_dir() {
            params.push(("DARWIN_USER_CACHE_DIR".to_string(), cache_dir.clone()));
            sbpl.push_str(&format!(
                "(allow file-write* (subpath (param \"DARWIN_USER_CACHE_DIR\")))\n"
            ));
        }

        (sbpl, params)
    }
}

impl Default for SeatbeltBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl SandboxBackend for SeatbeltBackend {
    fn name(&self) -> &str {
        "seatbelt"
    }

    fn is_available(&self) -> bool {
        self.sandbox_exec_path.exists()
    }

    fn prepare_command(
        &self,
        command: &[String],
        policy: &SandboxPolicyType,
        _cwd: &PathBuf,
        writable_roots: &[WritableRoot],
    ) -> Result<SandboxedCommand> {
        if command.is_empty() {
            return Ok(SandboxedCommand::passthrough(command));
        }

        let (sbpl, params) = self.generate_policy(policy, writable_roots);

        // Build sandbox-exec arguments
        let mut args = vec!["-p".to_string(), sbpl];

        // Add parameter definitions (-DNAME=VALUE)
        for (name, value) in &params {
            args.push(format!("-D{}={}", name, value));
        }

        // Add the actual command
        args.push("--".to_string());
        args.extend(command.iter().cloned());

        // Build environment variables
        let mut env = vec![(CORTEX_SANDBOX_ENV_VAR.to_string(), "seatbelt".to_string())];

        if !policy.has_full_network_access() {
            env.push((
                CORTEX_SANDBOX_NETWORK_DISABLED_ENV_VAR.to_string(),
                "1".to_string(),
            ));
        }

        Ok(SandboxedCommand {
            program: self.sandbox_exec_path.display().to_string(),
            args,
            env,
        })
    }
}

/// Get the Darwin user cache directory via confstr.
#[cfg(target_os = "macos")]
fn get_darwin_cache_dir() -> Option<String> {
    use std::ffi::CStr;

    let mut buf = vec![0i8; (libc::PATH_MAX as usize) + 1];
    let len =
        unsafe { libc::confstr(libc::_CS_DARWIN_USER_CACHE_DIR, buf.as_mut_ptr(), buf.len()) };

    if len == 0 {
        return None;
    }

    let cstr = unsafe { CStr::from_ptr(buf.as_ptr()) };
    cstr.to_str().ok().map(|s| {
        // Canonicalize the path if possible
        std::path::Path::new(s)
            .canonicalize()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| s.to_string())
    })
}

#[cfg(not(target_os = "macos"))]
fn get_darwin_cache_dir() -> Option<String> {
    None
}
