//! Windows Job Object implementation for process isolation.
//!
//! Job Objects provide:
//! - Process group management
//! - Resource limits (CPU, memory, I/O)
//! - UI restrictions (clipboard, display settings)
//! - Security isolation (no admin processes, breakaway restrictions)

use crate::{Result, WindowsSandboxError};
use std::ptr;
use tracing::{debug, warn};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicLimitInformation,
    JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation,
    JobObjectSecurityLimitInformation, QueryInformationJobObject, SetInformationJobObject,
    JOBOBJECT_BASIC_LIMIT_INFORMATION, JOBOBJECT_BASIC_UI_RESTRICTIONS,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOBOBJECT_SECURITY_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
    JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_JOB_MEMORY,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    JOB_OBJECT_LIMIT_PROCESS_TIME, JOB_OBJECT_SECURITY_FILTER_TOKENS, JOB_OBJECT_SECURITY_NO_ADMIN,
    JOB_OBJECT_UILIMIT_DESKTOP, JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS,
    JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES, JOB_OBJECT_UILIMIT_READCLIPBOARD,
    JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS, JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
};
use windows::Win32::System::Threading::GetCurrentProcess;

/// Configuration for Job Object limits.
#[derive(Debug, Clone)]
pub struct JobLimits {
    /// Maximum number of active processes (0 = unlimited).
    pub max_active_processes: u32,

    /// Per-process memory limit in bytes (0 = unlimited).
    pub per_process_memory_limit: usize,

    /// Total job memory limit in bytes (0 = unlimited).
    pub job_memory_limit: usize,

    /// Per-process user-mode time limit in 100ns units (0 = unlimited).
    pub per_process_time_limit: u64,

    /// Kill all processes when job handle is closed.
    pub kill_on_close: bool,

    /// Die on unhandled exception.
    pub die_on_unhandled_exception: bool,

    /// Prevent breakaway from job (children inherit job).
    pub prevent_breakaway: bool,

    /// Restrict UI operations (clipboard, display, etc.).
    pub restrict_ui: bool,

    /// Block admin token processes.
    pub no_admin: bool,
}

impl Default for JobLimits {
    fn default() -> Self {
        Self {
            max_active_processes: 0,     // Unlimited
            per_process_memory_limit: 0, // Unlimited
            job_memory_limit: 0,         // Unlimited
            per_process_time_limit: 0,   // Unlimited
            kill_on_close: true,
            die_on_unhandled_exception: true,
            prevent_breakaway: true,
            restrict_ui: true,
            no_admin: true,
        }
    }
}

impl JobLimits {
    /// Create restrictive limits for sandbox use.
    pub fn restrictive() -> Self {
        Self {
            max_active_processes: 100,
            per_process_memory_limit: 2 * 1024 * 1024 * 1024, // 2 GB
            job_memory_limit: 4 * 1024 * 1024 * 1024,         // 4 GB
            per_process_time_limit: 0,                        // No CPU limit
            kill_on_close: true,
            die_on_unhandled_exception: true,
            prevent_breakaway: true,
            restrict_ui: true,
            no_admin: true,
        }
    }

    /// Create minimal limits (primarily for process tracking).
    pub fn minimal() -> Self {
        Self {
            max_active_processes: 0,
            per_process_memory_limit: 0,
            job_memory_limit: 0,
            per_process_time_limit: 0,
            kill_on_close: true,
            die_on_unhandled_exception: false,
            prevent_breakaway: false,
            restrict_ui: false,
            no_admin: false,
        }
    }
}

/// Windows Job Object wrapper for process isolation.
///
/// Job Objects group processes together and apply resource limits
/// and security restrictions to all processes in the job.
pub struct JobObject {
    handle: HANDLE,
    limits: JobLimits,
}

impl JobObject {
    /// Create a new Job Object with the specified limits.
    pub fn new(limits: JobLimits) -> Result<Self> {
        // Create the job object with no name and default security
        let handle = unsafe { CreateJobObjectW(None, PCWSTR::null()) }
            .map_err(|e| WindowsSandboxError::JobObjectFailed(format!("CreateJobObjectW: {e}")))?;

        if handle.is_invalid() {
            return Err(WindowsSandboxError::JobObjectFailed(
                "CreateJobObjectW returned invalid handle".to_string(),
            ));
        }

        debug!("Created Job Object handle: {:?}", handle);

        let mut job = Self { handle, limits };
        job.apply_limits()?;

        Ok(job)
    }

    /// Create a Job Object with default restrictive limits.
    pub fn restrictive() -> Result<Self> {
        Self::new(JobLimits::restrictive())
    }

    /// Create a Job Object with minimal limits.
    pub fn minimal() -> Result<Self> {
        Self::new(JobLimits::minimal())
    }

    /// Apply the configured limits to the Job Object.
    fn apply_limits(&mut self) -> Result<()> {
        self.apply_basic_limits()?;
        self.apply_extended_limits()?;

        if self.limits.restrict_ui {
            self.apply_ui_restrictions()?;
        }

        if self.limits.no_admin {
            self.apply_security_limits()?;
        }

        Ok(())
    }

    /// Apply basic process limits.
    fn apply_basic_limits(&self) -> Result<()> {
        let mut info: JOBOBJECT_BASIC_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        let mut limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

        if self.limits.kill_on_close {
            limit_flags |= JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        }

        if self.limits.die_on_unhandled_exception {
            limit_flags |= JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
        }

        if !self.limits.prevent_breakaway {
            limit_flags |= JOB_OBJECT_LIMIT_BREAKAWAY_OK;
        }

        if self.limits.max_active_processes > 0 {
            limit_flags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
            info.ActiveProcessLimit = self.limits.max_active_processes;
        }

        if self.limits.per_process_time_limit > 0 {
            limit_flags |= JOB_OBJECT_LIMIT_PROCESS_TIME;
            info.PerProcessUserTimeLimit = self.limits.per_process_time_limit as i64;
        }

        info.LimitFlags = limit_flags;

        let result = unsafe {
            SetInformationJobObject(
                self.handle,
                JobObjectBasicLimitInformation,
                ptr::addr_of!(info) as *const _,
                std::mem::size_of::<JOBOBJECT_BASIC_LIMIT_INFORMATION>() as u32,
            )
        };

        if let Err(e) = result {
            warn!("Failed to set basic job limits: {}", e);
            return Err(WindowsSandboxError::JobObjectFailed(format!(
                "SetInformationJobObject(BasicLimit): {e}"
            )));
        }

        debug!("Applied basic job limits: flags={:x}", limit_flags.0);
        Ok(())
    }

    /// Apply extended limits including memory constraints.
    fn apply_extended_limits(&self) -> Result<()> {
        // First query existing settings
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        let mut returned_length: u32 = 0;

        let query_result = unsafe {
            QueryInformationJobObject(
                self.handle,
                JobObjectExtendedLimitInformation,
                ptr::addr_of_mut!(info) as *mut _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                Some(&mut returned_length),
            )
        };

        if query_result.is_err() {
            // Initialize fresh if query fails
            info = unsafe { std::mem::zeroed() };
        }

        let mut limit_flags = info.BasicLimitInformation.LimitFlags;

        if self.limits.per_process_memory_limit > 0 {
            limit_flags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
            info.ProcessMemoryLimit = self.limits.per_process_memory_limit;
        }

        if self.limits.job_memory_limit > 0 {
            limit_flags |= JOB_OBJECT_LIMIT_JOB_MEMORY;
            info.JobMemoryLimit = self.limits.job_memory_limit;
        }

        info.BasicLimitInformation.LimitFlags = limit_flags;

        let result = unsafe {
            SetInformationJobObject(
                self.handle,
                JobObjectExtendedLimitInformation,
                ptr::addr_of!(info) as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };

        if let Err(e) = result {
            warn!("Failed to set extended job limits: {}", e);
            return Err(WindowsSandboxError::JobObjectFailed(format!(
                "SetInformationJobObject(ExtendedLimit): {e}"
            )));
        }

        debug!("Applied extended job limits");
        Ok(())
    }

    /// Apply UI restrictions (clipboard, display, etc.).
    fn apply_ui_restrictions(&self) -> Result<()> {
        let mut info: JOBOBJECT_BASIC_UI_RESTRICTIONS = unsafe { std::mem::zeroed() };

        // Apply comprehensive UI restrictions for sandboxing
        info.UIRestrictionsClass = JOB_OBJECT_UILIMIT_DESKTOP
            | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
            | JOB_OBJECT_UILIMIT_EXITWINDOWS
            | JOB_OBJECT_UILIMIT_GLOBALATOMS
            | JOB_OBJECT_UILIMIT_HANDLES
            | JOB_OBJECT_UILIMIT_READCLIPBOARD
            | JOB_OBJECT_UILIMIT_WRITECLIPBOARD
            | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS;

        let result = unsafe {
            SetInformationJobObject(
                self.handle,
                JobObjectBasicUIRestrictions,
                ptr::addr_of!(info) as *const _,
                std::mem::size_of::<JOBOBJECT_BASIC_UI_RESTRICTIONS>() as u32,
            )
        };

        if let Err(e) = result {
            // UI restrictions may fail if not running in interactive session
            warn!(
                "Failed to set UI restrictions (may be non-interactive): {}",
                e
            );
        } else {
            debug!("Applied UI restrictions");
        }

        Ok(())
    }

    /// Apply security limits (no admin tokens, filter tokens).
    fn apply_security_limits(&self) -> Result<()> {
        let mut info: JOBOBJECT_SECURITY_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };

        info.SecurityLimitFlags = JOB_OBJECT_SECURITY_NO_ADMIN | JOB_OBJECT_SECURITY_FILTER_TOKENS;

        let result = unsafe {
            SetInformationJobObject(
                self.handle,
                JobObjectSecurityLimitInformation,
                ptr::addr_of!(info) as *const _,
                std::mem::size_of::<JOBOBJECT_SECURITY_LIMIT_INFORMATION>() as u32,
            )
        };

        if let Err(e) = result {
            // Security limits may require specific permissions
            warn!("Failed to set security limits: {}", e);
        } else {
            debug!("Applied security limits");
        }

        Ok(())
    }

    /// Assign the current process to this Job Object.
    pub fn assign_current_process(&self) -> Result<()> {
        let process = unsafe { GetCurrentProcess() };
        self.assign_process(process)
    }

    /// Assign a process to this Job Object.
    pub fn assign_process(&self, process_handle: HANDLE) -> Result<()> {
        let result = unsafe { AssignProcessToJobObject(self.handle, process_handle) };

        if let Err(e) = result {
            // Error 5 (ACCESS_DENIED) often means process is already in a job
            return Err(WindowsSandboxError::JobObjectFailed(format!(
                "AssignProcessToJobObject: {e}"
            )));
        }

        debug!("Assigned process to job object");
        Ok(())
    }

    /// Get the raw handle to the Job Object.
    pub fn handle(&self) -> HANDLE {
        self.handle
    }

    /// Check if Job Objects are available on this system.
    pub fn is_available() -> bool {
        // Job Objects are available on all supported Windows versions
        // Try creating a temporary one to verify
        match Self::minimal() {
            Ok(_) => true,
            Err(e) => {
                debug!("Job Objects not available: {}", e);
                false
            }
        }
    }
}

impl Drop for JobObject {
    fn drop(&mut self) {
        if !self.handle.is_invalid() {
            let result = unsafe { CloseHandle(self.handle) };
            if result.is_err() {
                warn!("Failed to close Job Object handle");
            }
        }
    }
}

// Safety: Job Object handles can be sent between threads
unsafe impl Send for JobObject {}
unsafe impl Sync for JobObject {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_limits_default() {
        let limits = JobLimits::default();
        assert!(limits.kill_on_close);
        assert!(limits.no_admin);
    }

    #[test]
    fn test_job_limits_restrictive() {
        let limits = JobLimits::restrictive();
        assert_eq!(limits.max_active_processes, 100);
        assert!(limits.restrict_ui);
    }
}
