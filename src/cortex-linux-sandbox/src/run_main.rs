//! Main entry point for the Linux sandbox wrapper.

use std::ffi::CString;
use std::path::PathBuf;

use clap::Parser;
use serde::{Deserialize, Serialize};

use crate::landlock::apply_filesystem_rules;
use crate::mounts::apply_read_only_mounts;
use crate::seccomp::apply_network_filter;

/// Sandbox policy type (simplified for the wrapper).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SandboxPolicy {
    DangerFullAccess,
    ReadOnly,
    WorkspaceWrite {
        #[serde(default)]
        additional_writable: Vec<PathBuf>,
        #[serde(default = "default_true")]
        network_access: bool,
    },
    Custom {
        writable_roots: Vec<WritableRoot>,
        #[serde(default)]
        network_access: bool,
    },
}

fn default_true() -> bool {
    true
}

impl SandboxPolicy {
    pub fn has_full_disk_write_access(&self) -> bool {
        matches!(self, Self::DangerFullAccess)
    }

    pub fn has_full_network_access(&self) -> bool {
        match self {
            Self::DangerFullAccess => true,
            Self::ReadOnly => false,
            Self::WorkspaceWrite { network_access, .. } => *network_access,
            Self::Custom { network_access, .. } => *network_access,
        }
    }
}

/// Writable root with read-only subpaths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WritableRoot {
    pub root: PathBuf,
    #[serde(default)]
    pub read_only_subpaths: Vec<PathBuf>,
}

/// Command line arguments for the sandbox wrapper.
#[derive(Debug, Parser)]
#[clap(name = "cortex-linux-sandbox", about = "Linux sandbox wrapper")]
pub struct SandboxArgs {
    /// The cwd used for computing sandbox policy paths.
    #[arg(long = "sandbox-policy-cwd")]
    pub sandbox_policy_cwd: PathBuf,

    /// The sandbox policy as JSON.
    #[arg(long = "sandbox-policy")]
    pub sandbox_policy: String,

    /// Additional writable root paths.
    #[arg(long = "writable-root")]
    pub writable_roots: Vec<PathBuf>,

    /// Read-only subpaths (within writable roots).
    #[arg(long = "read-only-subpath")]
    pub read_only_subpaths: Vec<PathBuf>,

    /// Command and arguments to execute.
    #[arg(trailing_var_arg = true, required = true)]
    pub command: Vec<String>,
}

/// Main entry point.
pub fn run_main() -> ! {
    let args = SandboxArgs::parse();

    // Parse the sandbox policy
    let policy: SandboxPolicy = match serde_json::from_str(&args.sandbox_policy) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Failed to parse sandbox policy: {}", e);
            std::process::exit(1);
        }
    };

    // Build writable roots from command line args
    let writable_roots: Vec<WritableRoot> = if args.writable_roots.is_empty() {
        // Use cwd as the writable root with standard protections
        vec![WritableRoot {
            root: args.sandbox_policy_cwd.clone(),
            read_only_subpaths: vec![
                args.sandbox_policy_cwd.join(".git"),
                args.sandbox_policy_cwd.join(".cortex"),
            ],
        }]
    } else {
        args.writable_roots
            .into_iter()
            .map(|root| WritableRoot {
                root,
                read_only_subpaths: args.read_only_subpaths.clone(),
            })
            .collect()
    };

    // Apply sandbox policy
    if let Err(e) = apply_sandbox_policy(&policy, &args.sandbox_policy_cwd, &writable_roots) {
        eprintln!("Failed to apply sandbox policy: {}", e);
        std::process::exit(1);
    }

    // Execute the command
    if args.command.is_empty() {
        eprintln!("No command specified");
        std::process::exit(1);
    }

    exec_command(&args.command)
}

/// Apply the sandbox policy to the current process.
fn apply_sandbox_policy(
    policy: &SandboxPolicy,
    _cwd: &PathBuf,
    writable_roots: &[WritableRoot],
) -> anyhow::Result<()> {
    // Skip if full access
    if policy.has_full_disk_write_access() {
        return Ok(());
    }

    // Set no_new_privs first
    set_no_new_privs()?;

    // Apply read-only mounts for protected subpaths (before Landlock)
    let read_only_paths: Vec<PathBuf> = writable_roots
        .iter()
        .flat_map(|r| r.read_only_subpaths.clone())
        .filter(|p| p.exists())
        .collect();

    if !read_only_paths.is_empty() {
        if let Err(e) = apply_read_only_mounts(&read_only_paths) {
            // Non-fatal: may fail without user namespace support
            tracing::warn!("Could not apply read-only mounts: {}", e);
        }
    }

    // Apply network filter (seccomp) if network is disabled
    if !policy.has_full_network_access() {
        apply_network_filter()?;
    }

    // Apply filesystem rules (Landlock)
    let writable_paths: Vec<PathBuf> = writable_roots.iter().map(|r| r.root.clone()).collect();
    apply_filesystem_rules(&writable_paths)?;

    Ok(())
}

/// Set PR_SET_NO_NEW_PRIVS.
fn set_no_new_privs() -> anyhow::Result<()> {
    let result = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

/// Execute the command using execvp.
fn exec_command(command: &[String]) -> ! {
    let c_command = match CString::new(command[0].as_str()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Invalid command: {}", e);
            std::process::exit(1);
        }
    };

    let c_args: Vec<CString> = command
        .iter()
        .map(|arg| CString::new(arg.as_str()).expect("Invalid argument"))
        .collect();

    let mut c_args_ptrs: Vec<*const libc::c_char> = c_args.iter().map(|arg| arg.as_ptr()).collect();
    c_args_ptrs.push(std::ptr::null());

    unsafe {
        libc::execvp(c_command.as_ptr(), c_args_ptrs.as_ptr());
    }

    // If execvp returns, there was an error
    let err = std::io::Error::last_os_error();
    eprintln!("Failed to execute {}: {}", command[0], err);
    std::process::exit(127)
}
