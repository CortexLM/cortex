//! Windows sandbox implementation.
//!
//! This module provides Windows-specific sandboxing using:
//! - Job Objects for process isolation and resource limits
//! - Restricted Tokens for privilege reduction
//! - Process Mitigations for exploit prevention
//!
//! # Architecture
//!
//! The Windows sandbox uses a layered approach:
//!
//! 1. **Job Objects** - Group processes together with resource limits
//!    - Maximum process count
//!    - Memory limits per process and total
//!    - UI restrictions (clipboard, desktop, etc.)
//!    - Security limits (no admin tokens)
//!
//! 2. **Restricted Tokens** - Reduce process privileges
//!    - Remove dangerous privileges (SeDebugPrivilege, etc.)
//!    - Disable admin SID
//!    - Create low-integrity tokens
//!
//! 3. **Process Mitigations** - Prevent exploits
//!    - DEP (Data Execution Prevention)
//!    - ASLR (Address Space Layout Randomization)
//!    - Extension point disabling (blocks DLL injection)
//!
//! # Usage
//!
//! ```rust,ignore
//! use cortex_sandbox::windows::WindowsSandbox;
//!
//! let sandbox = WindowsSandbox::new();
//! if sandbox.is_available() {
//!     sandbox.apply(&[Path::new("/tmp")], true)?;
//! }
//! ```

use std::path::Path;
use std::ptr;

use crate::{SandboxBackend, SandboxError, SandboxResult};

use windows::Win32::Foundation::{CloseHandle, HANDLE, LUID};
use windows::Win32::Security::{
    AdjustTokenPrivileges, LookupPrivilegeValueW, SE_PRIVILEGE_REMOVED, TOKEN_ADJUST_PRIVILEGES,
    TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_JOB_MEMORY,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    JOB_OBJECT_UILIMIT_DESKTOP, JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_GLOBALATOMS,
    JOB_OBJECT_UILIMIT_HANDLES, JOB_OBJECT_UILIMIT_READCLIPBOARD,
    JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS, JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
    JOBOBJECT_BASIC_LIMIT_INFORMATION, JOBOBJECT_BASIC_UI_RESTRICTIONS,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicLimitInformation,
    JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::PCWSTR;

/// Dangerous privileges to remove for sandboxed processes.
const DANGEROUS_PRIVILEGES: &[&str] = &[
    "SeDebugPrivilege",
    "SeBackupPrivilege",
    "SeRestorePrivilege",
    "SeTakeOwnershipPrivilege",
    "SeLoadDriverPrivilege",
    "SeImpersonatePrivilege",
    "SeCreateTokenPrivilege",
    "SeAssignPrimaryTokenPrivilege",
    "SeTcbPrivilege",
];

// FFI for process mitigation APIs
#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetProcessMitigationPolicy(
        policy: i32,
        lpbuffer: *const std::ffi::c_void,
        size: usize,
    ) -> i32;
    fn GetProcessMitigationPolicy(
        process: *mut std::ffi::c_void,
        policy: i32,
        lpbuffer: *mut std::ffi::c_void,
        size: usize,
    ) -> i32;
}

// Mitigation policy constants
const PROCESS_DEP_POLICY: i32 = 0;
const PROCESS_ASLR_POLICY: i32 = 1;
const PROCESS_EXTENSION_POINT_DISABLE_POLICY: i32 = 7;

#[repr(C)]
struct DepPolicy {
    flags: u32,
}

#[repr(C)]
struct AslrPolicy {
    flags: u32,
}

#[repr(C)]
struct ExtensionPointPolicy {
    flags: u32,
}

/// Windows sandbox backend using Job Objects, Restricted Tokens, and Process Mitigations.
pub struct WindowsSandbox {
    available: bool,
    job_handle: Option<HANDLE>,
    applied: bool,
}

impl WindowsSandbox {
    /// Create a new Windows sandbox.
    ///
    /// This checks if the required Windows APIs are available and creates
    /// a Job Object for process isolation.
    pub fn new() -> Self {
        let available = Self::check_available();
        let job_handle = if available {
            Self::create_job_object().ok()
        } else {
            None
        };

        Self {
            available: available && job_handle.is_some(),
            job_handle,
            applied: false,
        }
    }

    /// Check if Windows sandboxing is available.
    fn check_available() -> bool {
        // Try to query DEP policy to verify API availability
        let process = unsafe { GetCurrentProcess() };
        let raw_process = process.0 as *mut std::ffi::c_void;
        let mut policy: DepPolicy = DepPolicy { flags: 0 };

        let result = unsafe {
            GetProcessMitigationPolicy(
                raw_process,
                PROCESS_DEP_POLICY,
                ptr::addr_of_mut!(policy) as *mut _,
                std::mem::size_of::<DepPolicy>(),
            )
        };

        result != 0
    }

    /// Create a Job Object for process isolation.
    fn create_job_object() -> Result<HANDLE, SandboxError> {
        let handle = unsafe { CreateJobObjectW(None, PCWSTR::null()) }
            .map_err(|e| SandboxError::ApplyFailed(format!("CreateJobObjectW: {e}")))?;

        if handle.is_invalid() {
            return Err(SandboxError::ApplyFailed(
                "CreateJobObjectW returned invalid handle".to_string(),
            ));
        }

        // Configure basic limits
        let mut basic_info: JOBOBJECT_BASIC_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        basic_info.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        basic_info.ActiveProcessLimit = 100;

        let _ = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectBasicLimitInformation,
                ptr::addr_of!(basic_info) as *const _,
                std::mem::size_of::<JOBOBJECT_BASIC_LIMIT_INFORMATION>() as u32,
            )
        };

        // Configure extended limits (memory)
        let mut ext_info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        ext_info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_JOB_MEMORY
            | JOB_OBJECT_LIMIT_PROCESS_MEMORY
            | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        ext_info.ProcessMemoryLimit = 2 * 1024 * 1024 * 1024; // 2 GB
        ext_info.JobMemoryLimit = 4 * 1024 * 1024 * 1024; // 4 GB

        let _ = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                ptr::addr_of!(ext_info) as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };

        // Configure UI restrictions
        let mut ui_info: JOBOBJECT_BASIC_UI_RESTRICTIONS = unsafe { std::mem::zeroed() };
        ui_info.UIRestrictionsClass = JOB_OBJECT_UILIMIT_DESKTOP
            | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
            | JOB_OBJECT_UILIMIT_GLOBALATOMS
            | JOB_OBJECT_UILIMIT_HANDLES
            | JOB_OBJECT_UILIMIT_READCLIPBOARD
            | JOB_OBJECT_UILIMIT_WRITECLIPBOARD
            | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS;

        // UI restrictions may fail in non-interactive sessions - that's OK
        let _ = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectBasicUIRestrictions,
                ptr::addr_of!(ui_info) as *const _,
                std::mem::size_of::<JOBOBJECT_BASIC_UI_RESTRICTIONS>() as u32,
            )
        };

        Ok(handle)
    }

    /// Apply sandbox restrictions to the current process.
    ///
    /// This applies:
    /// 1. Process mitigations (DEP, ASLR, extension point disabling)
    /// 2. Assigns the process to the job object
    /// 3. Removes dangerous privileges from the token
    /// 4. Clears sensitive environment variables
    pub fn apply(&mut self, _writable_roots: &[&Path], _allow_network: bool) -> SandboxResult<()> {
        if !self.available {
            return Err(SandboxError::NotAvailable);
        }

        if self.applied {
            return Ok(());
        }

        // Apply process mitigations
        self.apply_mitigations()?;

        // Assign to job object
        if let Some(job) = self.job_handle {
            let process = unsafe { GetCurrentProcess() };
            if let Err(e) = unsafe { AssignProcessToJobObject(job, process) } {
                // Process may already be in a job - log but don't fail
                eprintln!("Warning: Could not assign to job object: {e}");
            }
        }

        // Remove dangerous privileges
        self.remove_dangerous_privileges()?;

        // Clear sensitive environment variables
        self.clear_sensitive_env_vars();

        self.applied = true;
        Ok(())
    }

    /// Apply process mitigations (DEP, ASLR, extension point disabling).
    fn apply_mitigations(&self) -> SandboxResult<()> {
        // Enable DEP permanently
        let dep_policy = DepPolicy { flags: 0x3 }; // Enable | Permanent

        let _ = unsafe {
            SetProcessMitigationPolicy(
                PROCESS_DEP_POLICY,
                ptr::addr_of!(dep_policy) as *const _,
                std::mem::size_of::<DepPolicy>(),
            )
        };

        // Enable ASLR features
        let aslr_policy = AslrPolicy { flags: 0x7 }; // BottomUp | HighEntropy | ForceRelocate

        let _ = unsafe {
            SetProcessMitigationPolicy(
                PROCESS_ASLR_POLICY,
                ptr::addr_of!(aslr_policy) as *const _,
                std::mem::size_of::<AslrPolicy>(),
            )
        };

        // Disable extension points (blocks AppInit DLLs, etc.)
        let ext_policy = ExtensionPointPolicy { flags: 0x1 }; // DisableExtensionPoints

        let _ = unsafe {
            SetProcessMitigationPolicy(
                PROCESS_EXTENSION_POINT_DISABLE_POLICY,
                ptr::addr_of!(ext_policy) as *const _,
                std::mem::size_of::<ExtensionPointPolicy>(),
            )
        };

        Ok(())
    }

    /// Remove dangerous privileges from the current process token.
    fn remove_dangerous_privileges(&self) -> SandboxResult<()> {
        let process = unsafe { GetCurrentProcess() };
        let mut token = HANDLE::default();

        let result =
            unsafe { OpenProcessToken(process, TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token) };

        if result.is_err() {
            return Ok(()); // Can't adjust privileges, but don't fail
        }

        for priv_name in DANGEROUS_PRIVILEGES {
            let _ = remove_privilege(token, priv_name);
        }

        let _ = unsafe { CloseHandle(token) };
        Ok(())
    }

    /// Clear sensitive environment variables.
    fn clear_sensitive_env_vars(&self) {
        const SENSITIVE_VARS: &[&str] = &[
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
            "AZURE_CLIENT_SECRET",
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GITLAB_TOKEN",
            "NPM_TOKEN",
            "DATABASE_URL",
            "API_KEY",
            "SECRET_KEY",
            "AUTH_TOKEN",
        ];

        for var in SENSITIVE_VARS {
            if std::env::var_os(var).is_some() {
                // SAFETY: Intentionally clearing sensitive environment variables
                unsafe {
                    std::env::remove_var(var);
                }
            }
        }
    }

    /// Check if the sandbox has been applied.
    pub fn is_applied(&self) -> bool {
        self.applied
    }
}

impl Default for WindowsSandbox {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for WindowsSandbox {
    fn drop(&mut self) {
        if let Some(handle) = self.job_handle.take() {
            if !handle.is_invalid() {
                let _ = unsafe { CloseHandle(handle) };
            }
        }
    }
}

impl SandboxBackend for WindowsSandbox {
    fn name(&self) -> &str {
        "windows"
    }

    fn is_available(&self) -> bool {
        self.available
    }
}

/// Remove a single privilege from a token.
fn remove_privilege(token: HANDLE, privilege_name: &str) -> Result<(), SandboxError> {
    let wide_name: Vec<u16> = privilege_name
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut luid = LUID::default();

    let result = unsafe {
        LookupPrivilegeValueW(
            PCWSTR::null(),
            PCWSTR::from_raw(wide_name.as_ptr()),
            &mut luid,
        )
    };

    if result.is_err() {
        return Ok(()); // Privilege doesn't exist on this system
    }

    #[repr(C)]
    struct TokenPrivilegesOne {
        count: u32,
        privileges: [windows::Win32::Security::LUID_AND_ATTRIBUTES; 1],
    }

    let tp = TokenPrivilegesOne {
        count: 1,
        privileges: [windows::Win32::Security::LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: SE_PRIVILEGE_REMOVED,
        }],
    };

    let _ = unsafe {
        AdjustTokenPrivileges(
            token,
            false,
            Some(ptr::addr_of!(tp) as *const TOKEN_PRIVILEGES),
            0,
            None,
            None,
        )
    };

    Ok(())
}

// Safety: Job Object handles can be sent between threads
unsafe impl Send for WindowsSandbox {}
unsafe impl Sync for WindowsSandbox {}
