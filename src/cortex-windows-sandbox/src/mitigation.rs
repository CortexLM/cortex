//! Windows Process Mitigation Policies for exploit prevention.
//!
//! Process mitigation policies provide defense-in-depth by:
//! - Enabling DEP (Data Execution Prevention)
//! - Enabling ASLR (Address Space Layout Randomization)
//! - Disabling Win32k system calls (attack surface reduction)
//! - Enabling Control Flow Guard
//! - Preventing dynamic code generation

use crate::{Result, WindowsSandboxError};
use std::mem;
use std::ptr;
use tracing::{debug, info, warn};

// Windows API constants for mitigation policies
const PROCESS_DEP_POLICY: i32 = 0;
const PROCESS_ASLR_POLICY: i32 = 1;
const PROCESS_DYNAMIC_CODE_POLICY: i32 = 2;
const PROCESS_EXTENSION_POINT_DISABLE_POLICY: i32 = 7;
const PROCESS_IMAGE_LOAD_POLICY: i32 = 10;
const PROCESS_SYSTEM_CALL_DISABLE_POLICY: i32 = 6;

// Policy structure definitions (matching Windows SDK)
#[repr(C)]
struct DepPolicy {
    flags: u32,
}

#[repr(C)]
struct AslrPolicy {
    flags: u32,
}

#[repr(C)]
struct DynamicCodePolicy {
    flags: u32,
}

#[repr(C)]
struct ExtensionPointDisablePolicy {
    flags: u32,
}

#[repr(C)]
struct ImageLoadPolicy {
    flags: u32,
}

#[repr(C)]
struct SystemCallDisablePolicy {
    flags: u32,
}

// FFI declarations
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess() -> *mut std::ffi::c_void;
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

/// Configuration for process mitigation policies.
#[derive(Debug, Clone)]
pub struct MitigationConfig {
    /// Enable permanent DEP (Data Execution Prevention).
    pub enable_dep: bool,

    /// Enable ASLR features (high entropy, force relocation).
    pub enable_aslr: bool,

    /// Disable dynamic code (JIT, etc.).
    pub disable_dynamic_code: bool,

    /// Disable Win32k system calls (reduces attack surface).
    pub disable_win32k_syscalls: bool,

    /// Disable extension points (AppInit DLLs, etc.).
    pub disable_extension_points: bool,

    /// Require signed images (Microsoft signed only).
    pub require_signed_images: bool,

    /// Prefer images from System32.
    pub prefer_system32_images: bool,
}

impl Default for MitigationConfig {
    fn default() -> Self {
        Self {
            enable_dep: true,
            enable_aslr: true,
            disable_dynamic_code: false,    // May break some tools
            disable_win32k_syscalls: false, // May break console apps
            disable_extension_points: true,
            require_signed_images: false, // Too restrictive for dev tools
            prefer_system32_images: true,
        }
    }
}

impl MitigationConfig {
    /// Maximum security configuration.
    pub fn maximum_security() -> Self {
        Self {
            enable_dep: true,
            enable_aslr: true,
            disable_dynamic_code: true,
            disable_win32k_syscalls: true,
            disable_extension_points: true,
            require_signed_images: true,
            prefer_system32_images: true,
        }
    }

    /// Balanced security configuration suitable for CLI tools.
    pub fn balanced() -> Self {
        Self::default()
    }

    /// Minimal mitigations (mostly for compatibility).
    pub fn minimal() -> Self {
        Self {
            enable_dep: true,
            enable_aslr: true,
            disable_dynamic_code: false,
            disable_win32k_syscalls: false,
            disable_extension_points: false,
            require_signed_images: false,
            prefer_system32_images: false,
        }
    }
}

/// Process mitigation policies manager.
///
/// Applies Windows process mitigation policies for exploit prevention.
pub struct ProcessMitigations {
    config: MitigationConfig,
    applied: bool,
}

impl ProcessMitigations {
    /// Create a new ProcessMitigations manager with the given configuration.
    pub fn new(config: MitigationConfig) -> Self {
        Self {
            config,
            applied: false,
        }
    }

    /// Create with balanced (default) configuration.
    pub fn balanced() -> Self {
        Self::new(MitigationConfig::balanced())
    }

    /// Create with maximum security configuration.
    pub fn maximum_security() -> Self {
        Self::new(MitigationConfig::maximum_security())
    }

    /// Apply all configured mitigations to the current process.
    ///
    /// Note: Most mitigations can only be enabled, not disabled once set.
    pub fn apply(&mut self) -> Result<()> {
        let mut successful = 0;
        let mut total = 0;

        if self.config.enable_dep {
            total += 1;
            if enable_dep().is_ok() {
                successful += 1;
            } else {
                warn!("Failed to enable DEP");
            }
        }

        if self.config.enable_aslr {
            total += 1;
            if enable_aslr().is_ok() {
                successful += 1;
            } else {
                warn!("Failed to enable ASLR");
            }
        }

        if self.config.disable_dynamic_code {
            total += 1;
            if disable_dynamic_code().is_ok() {
                successful += 1;
            } else {
                warn!("Failed to disable dynamic code");
            }
        }

        if self.config.disable_win32k_syscalls {
            total += 1;
            if disable_win32k_syscalls().is_ok() {
                successful += 1;
            } else {
                warn!("Failed to disable Win32k syscalls");
            }
        }

        if self.config.disable_extension_points {
            total += 1;
            if disable_extension_points().is_ok() {
                successful += 1;
            } else {
                warn!("Failed to disable extension points");
            }
        }

        if self.config.prefer_system32_images {
            total += 1;
            if enable_image_load_policy().is_ok() {
                successful += 1;
            } else {
                warn!("Failed to set image load policy");
            }
        }

        self.applied = true;
        info!(
            "Applied {}/{} process mitigation policies",
            successful, total
        );

        Ok(())
    }

    /// Check if mitigations have been applied.
    pub fn is_applied(&self) -> bool {
        self.applied
    }

    /// Check if process mitigations are available on this system.
    pub fn is_available() -> bool {
        // Try querying DEP policy to check if API is available
        let process = unsafe { GetCurrentProcess() };
        let mut policy: DepPolicy = unsafe { mem::zeroed() };

        let result = unsafe {
            GetProcessMitigationPolicy(
                process,
                PROCESS_DEP_POLICY,
                ptr::addr_of_mut!(policy) as *mut _,
                mem::size_of::<DepPolicy>(),
            )
        };

        result != 0
    }
}

/// Enable DEP (Data Execution Prevention) permanently.
fn enable_dep() -> Result<()> {
    let policy = DepPolicy {
        flags: 0x3, // Enable | Permanent
    };

    let result = unsafe {
        SetProcessMitigationPolicy(
            PROCESS_DEP_POLICY,
            ptr::addr_of!(policy) as *const _,
            mem::size_of::<DepPolicy>(),
        )
    };

    if result == 0 {
        return Err(WindowsSandboxError::MitigationFailed("DEP".to_string()));
    }

    debug!("Enabled DEP permanently");
    Ok(())
}

/// Enable ASLR (Address Space Layout Randomization) features.
fn enable_aslr() -> Result<()> {
    let policy = AslrPolicy {
        flags: 0x7, // EnableBottomUpRandomization | EnableHighEntropy | EnableForceRelocateImages
    };

    let result = unsafe {
        SetProcessMitigationPolicy(
            PROCESS_ASLR_POLICY,
            ptr::addr_of!(policy) as *const _,
            mem::size_of::<AslrPolicy>(),
        )
    };

    if result == 0 {
        return Err(WindowsSandboxError::MitigationFailed("ASLR".to_string()));
    }

    debug!("Enabled ASLR features");
    Ok(())
}

/// Disable dynamic code generation (blocks JIT, etc.).
fn disable_dynamic_code() -> Result<()> {
    let policy = DynamicCodePolicy {
        flags: 0x1, // ProhibitDynamicCode
    };

    let result = unsafe {
        SetProcessMitigationPolicy(
            PROCESS_DYNAMIC_CODE_POLICY,
            ptr::addr_of!(policy) as *const _,
            mem::size_of::<DynamicCodePolicy>(),
        )
    };

    if result == 0 {
        return Err(WindowsSandboxError::MitigationFailed(
            "DynamicCode".to_string(),
        ));
    }

    debug!("Disabled dynamic code");
    Ok(())
}

/// Disable Win32k system calls (reduces kernel attack surface).
fn disable_win32k_syscalls() -> Result<()> {
    let policy = SystemCallDisablePolicy {
        flags: 0x1, // DisallowWin32kSystemCalls
    };

    let result = unsafe {
        SetProcessMitigationPolicy(
            PROCESS_SYSTEM_CALL_DISABLE_POLICY,
            ptr::addr_of!(policy) as *const _,
            mem::size_of::<SystemCallDisablePolicy>(),
        )
    };

    if result == 0 {
        return Err(WindowsSandboxError::MitigationFailed(
            "Win32kSyscalls".to_string(),
        ));
    }

    debug!("Disabled Win32k system calls");
    Ok(())
}

/// Disable extension points (blocks AppInit DLLs and other injection vectors).
fn disable_extension_points() -> Result<()> {
    let policy = ExtensionPointDisablePolicy {
        flags: 0x1, // DisableExtensionPoints
    };

    let result = unsafe {
        SetProcessMitigationPolicy(
            PROCESS_EXTENSION_POINT_DISABLE_POLICY,
            ptr::addr_of!(policy) as *const _,
            mem::size_of::<ExtensionPointDisablePolicy>(),
        )
    };

    if result == 0 {
        return Err(WindowsSandboxError::MitigationFailed(
            "ExtensionPoints".to_string(),
        ));
    }

    debug!("Disabled extension points");
    Ok(())
}

/// Enable image load policy (prefer System32, restrict remote images).
fn enable_image_load_policy() -> Result<()> {
    let policy = ImageLoadPolicy {
        flags: 0x2, // PreferSystem32Images
    };

    let result = unsafe {
        SetProcessMitigationPolicy(
            PROCESS_IMAGE_LOAD_POLICY,
            ptr::addr_of!(policy) as *const _,
            mem::size_of::<ImageLoadPolicy>(),
        )
    };

    if result == 0 {
        return Err(WindowsSandboxError::MitigationFailed(
            "ImageLoadPolicy".to_string(),
        ));
    }

    debug!("Enabled image load policy");
    Ok(())
}

/// Apply standard mitigations to the current process.
///
/// This is a convenience function that applies balanced mitigations.
pub fn apply_standard_mitigations() -> Result<()> {
    let mut mitigations = ProcessMitigations::balanced();
    mitigations.apply()
}

/// Apply maximum security mitigations to the current process.
///
/// Warning: This may break some functionality (JIT, GUI, etc.).
pub fn apply_maximum_mitigations() -> Result<()> {
    let mut mitigations = ProcessMitigations::maximum_security();
    mitigations.apply()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mitigation_config_default() {
        let config = MitigationConfig::default();
        assert!(config.enable_dep);
        assert!(config.enable_aslr);
        assert!(!config.disable_dynamic_code);
    }

    #[test]
    fn test_mitigation_config_maximum() {
        let config = MitigationConfig::maximum_security();
        assert!(config.disable_dynamic_code);
        assert!(config.disable_win32k_syscalls);
    }
}
