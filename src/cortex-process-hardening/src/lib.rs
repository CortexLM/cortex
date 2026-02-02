//! Cortex Process Hardening - Security hardening for the Cortex CLI process.
//!
//! This module implements various process hardening measures:
//! - Disabling core dumps
//! - Preventing ptrace attachment
//! - Removing dangerous environment variables

#![allow(unsafe_code, clippy::print_stderr)]

/// Exit code when prctl fails on Linux.
#[cfg(any(target_os = "linux", target_os = "android"))]
const PRCTL_FAILED_EXIT_CODE: i32 = 5;

/// Exit code when ptrace(PT_DENY_ATTACH) fails on macOS.
#[cfg(target_os = "macos")]
const PTRACE_DENY_ATTACH_FAILED_EXIT_CODE: i32 = 6;

/// Exit code when setrlimit(RLIMIT_CORE) fails.
#[cfg(unix)]
const SET_RLIMIT_CORE_FAILED_EXIT_CODE: i32 = 7;

/// Apply process hardening measures.
///
/// This function is designed to be called early in the process lifecycle,
/// typically using `#[ctor::ctor]` to run before main().
///
/// The hardening measures include:
/// - Disabling core dumps (prevents sensitive data from being written to disk)
/// - Preventing debugger attachment (prevents ptrace-based attacks)
/// - Removing dangerous environment variables (prevents library injection)
pub fn pre_main_hardening() {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pre_main_hardening_linux();

    #[cfg(target_os = "macos")]
    pre_main_hardening_macos();

    #[cfg(any(target_os = "freebsd", target_os = "openbsd"))]
    pre_main_hardening_bsd();

    #[cfg(windows)]
    pre_main_hardening_windows();
}

/// Linux-specific hardening.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn pre_main_hardening_linux() {
    // Disable ptrace attach / mark process non-dumpable
    let ret_code = unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) };
    if ret_code != 0 {
        eprintln!(
            "ERROR: prctl(PR_SET_DUMPABLE, 0) failed: {}",
            std::io::Error::last_os_error()
        );
        std::process::exit(PRCTL_FAILED_EXIT_CODE);
    }

    // Set core file size limit to 0
    set_core_file_size_limit_to_zero();

    // Clear LD_* environment variables
    clear_ld_env_vars();
}

/// macOS-specific hardening.
#[cfg(target_os = "macos")]
fn pre_main_hardening_macos() {
    // Prevent debuggers from attaching
    let ret_code = unsafe { libc::ptrace(libc::PT_DENY_ATTACH, 0, std::ptr::null_mut(), 0) };
    if ret_code == -1 {
        eprintln!(
            "ERROR: ptrace(PT_DENY_ATTACH) failed: {}",
            std::io::Error::last_os_error()
        );
        std::process::exit(PTRACE_DENY_ATTACH_FAILED_EXIT_CODE);
    }

    // Set core file size limit to 0
    set_core_file_size_limit_to_zero();

    // Remove DYLD_* environment variables
    clear_dyld_env_vars();
}

/// BSD-specific hardening (FreeBSD, OpenBSD).
#[cfg(any(target_os = "freebsd", target_os = "openbsd"))]
fn pre_main_hardening_bsd() {
    set_core_file_size_limit_to_zero();
    clear_ld_env_vars();
}

/// Windows-specific hardening.
///
/// Applies the following security measures:
/// 1. Checks for attached debuggers and exits if detected
/// 2. Applies process mitigation policies (DEP, ASLR, CFG)
/// 3. Disables DLL injection vectors (AppInit DLLs, extension points)
/// 4. Clears sensitive environment variables
#[cfg(windows)]
fn pre_main_hardening_windows() {
    // Exit code when debugger is detected
    const DEBUGGER_DETECTED_EXIT_CODE: i32 = 8;

    // 1. Check for debugger attachment
    if is_debugger_attached() {
        eprintln!("ERROR: Debugger detected - process terminating for security");
        std::process::exit(DEBUGGER_DETECTED_EXIT_CODE);
    }

    // 2. Apply process mitigation policies
    if let Err(e) = apply_windows_mitigations() {
        // Log but don't fail - mitigations are best-effort
        eprintln!("WARNING: Some process mitigations could not be applied: {e}");
    }

    // 3. Clear dangerous Windows environment variables
    clear_windows_env_vars();
}

/// Check if a debugger is attached to the current process.
#[cfg(windows)]
fn is_debugger_attached() -> bool {
    // Method 1: IsDebuggerPresent API
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn IsDebuggerPresent() -> i32;
        fn CheckRemoteDebuggerPresent(
            hProcess: *mut std::ffi::c_void,
            pbDebuggerPresent: *mut i32,
        ) -> i32;
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
    }

    // Check local debugger
    let local_debugger = unsafe { IsDebuggerPresent() != 0 };
    if local_debugger {
        return true;
    }

    // Check remote debugger
    let mut remote_debugger: i32 = 0;
    let process = unsafe { GetCurrentProcess() };
    let result = unsafe { CheckRemoteDebuggerPresent(process, &mut remote_debugger) };
    if result != 0 && remote_debugger != 0 {
        return true;
    }

    // Method 2: Check NtGlobalFlag in PEB (anti-debugging technique)
    // This detects debuggers that hide from IsDebuggerPresent
    if check_peb_being_debugged() {
        return true;
    }

    false
}

/// Check the PEB BeingDebugged flag directly.
/// This is an additional anti-debugging check that some debuggers don't hide.
#[cfg(windows)]
fn check_peb_being_debugged() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        // On x86_64, PEB is at gs:[0x60]
        let peb: *const u8;
        unsafe {
            std::arch::asm!(
                "mov {}, gs:[0x60]",
                out(reg) peb,
                options(nostack, preserves_flags)
            );
            // BeingDebugged is at offset 0x2 in PEB
            if !peb.is_null() {
                let being_debugged = *peb.add(0x2);
                return being_debugged != 0;
            }
        }
    }
    #[cfg(target_arch = "x86")]
    {
        // On x86, PEB is at fs:[0x30]
        let peb: *const u8;
        unsafe {
            std::arch::asm!(
                "mov {}, fs:[0x30]",
                out(reg) peb,
                options(nostack, preserves_flags)
            );
            // BeingDebugged is at offset 0x2 in PEB
            if !peb.is_null() {
                let being_debugged = *peb.add(0x2);
                return being_debugged != 0;
            }
        }
    }
    false
}

/// Apply Windows process mitigation policies.
#[cfg(windows)]
fn apply_windows_mitigations() -> Result<(), &'static str> {
    use std::mem;
    use std::ptr;

    // Define the necessary structures and constants
    #[repr(C)]
    struct ProcessMitigationDepPolicy {
        flags: u32,
    }

    #[repr(C)]
    struct ProcessMitigationAslrPolicy {
        flags: u32,
    }

    #[repr(C)]
    struct ProcessMitigationExtensionPointDisablePolicy {
        flags: u32,
    }

    #[repr(C)]
    struct ProcessMitigationImageLoadPolicy {
        flags: u32,
    }

    const PROCESS_DEP_POLICY: i32 = 0;
    const PROCESS_ASLR_POLICY: i32 = 1;
    const PROCESS_EXTENSION_POINT_DISABLE_POLICY: i32 = 7;
    const PROCESS_IMAGE_LOAD_POLICY: i32 = 10;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetProcessMitigationPolicy(
            policy_type: i32,
            policy: *const std::ffi::c_void,
            size: usize,
        ) -> i32;
    }

    let mut success_count = 0;

    // 1. Enable DEP (Data Execution Prevention) permanently
    let dep_policy = ProcessMitigationDepPolicy {
        flags: 0x3, // Enable | Permanent
    };
    let result = unsafe {
        SetProcessMitigationPolicy(
            PROCESS_DEP_POLICY,
            ptr::addr_of!(dep_policy) as *const _,
            mem::size_of::<ProcessMitigationDepPolicy>(),
        )
    };
    if result != 0 {
        success_count += 1;
    }

    // 2. Enable ASLR features
    let aslr_policy = ProcessMitigationAslrPolicy {
        flags: 0x7, // EnableBottomUpRandomization | EnableHighEntropy | EnableForceRelocateImages
    };
    let result = unsafe {
        SetProcessMitigationPolicy(
            PROCESS_ASLR_POLICY,
            ptr::addr_of!(aslr_policy) as *const _,
            mem::size_of::<ProcessMitigationAslrPolicy>(),
        )
    };
    if result != 0 {
        success_count += 1;
    }

    // 3. Disable extension points (blocks AppInit DLLs, shell extensions)
    let ext_policy = ProcessMitigationExtensionPointDisablePolicy {
        flags: 0x1, // DisableExtensionPoints
    };
    let result = unsafe {
        SetProcessMitigationPolicy(
            PROCESS_EXTENSION_POINT_DISABLE_POLICY,
            ptr::addr_of!(ext_policy) as *const _,
            mem::size_of::<ProcessMitigationExtensionPointDisablePolicy>(),
        )
    };
    if result != 0 {
        success_count += 1;
    }

    // 4. Prefer images from System32 (mitigates DLL planting)
    let image_policy = ProcessMitigationImageLoadPolicy {
        flags: 0x2, // PreferSystem32Images
    };
    let result = unsafe {
        SetProcessMitigationPolicy(
            PROCESS_IMAGE_LOAD_POLICY,
            ptr::addr_of!(image_policy) as *const _,
            mem::size_of::<ProcessMitigationImageLoadPolicy>(),
        )
    };
    if result != 0 {
        success_count += 1;
    }

    // Note: We intentionally do NOT disable dynamic code generation
    // as it may be needed by some tools the CLI invokes

    if success_count == 0 {
        return Err("No mitigation policies could be applied");
    }

    Ok(())
}

/// Clear dangerous Windows-specific environment variables.
///
/// These variables could be used for DLL injection or other attacks.
#[cfg(windows)]
fn clear_windows_env_vars() {
    // Environment variables that could enable DLL injection or hijacking
    const DANGEROUS_VARS: &[&str] = &[
        // DLL loading manipulation
        "APPINIT_DLLS",
        // Path manipulation
        "_NT_SYMBOL_PATH",
        "_NT_ALT_SYMBOL_PATH",
        // Debugging
        "_NT_DEBUG_LOG_FILE_APPEND",
        "_NT_DEBUG_LOG_FILE_OPEN",
        // JIT debugging
        "AeDebug",
        // VSS (Visual Studio) debugging
        "VS_DEBUG",
        // WER (Windows Error Reporting) settings
        "WER_DUMP_DISABLED",
        // Process creation hooks
        "COMPLUS_ENABLE_64BIT",
        // .NET profiling (can load arbitrary DLLs)
        "COR_ENABLE_PROFILING",
        "COR_PROFILER",
        "COR_PROFILER_PATH",
        "CORECLR_ENABLE_PROFILING",
        "CORECLR_PROFILER",
        "CORECLR_PROFILER_PATH",
        // Sensitive credentials
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "AZURE_CLIENT_SECRET",
        "AZURE_CLIENT_ID",
        "AZURE_TENANT_ID",
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "GITLAB_TOKEN",
        "NPM_TOKEN",
        "NUGET_API_KEY",
        "DATABASE_URL",
        "DB_PASSWORD",
        "API_KEY",
        "SECRET_KEY",
        "PRIVATE_KEY",
        "AUTH_TOKEN",
        "ACCESS_TOKEN",
    ];

    for var in DANGEROUS_VARS {
        if std::env::var_os(var).is_some() {
            // SAFETY: We're intentionally clearing potentially dangerous
            // environment variables that could enable attacks
            unsafe {
                std::env::remove_var(var);
            }
        }
    }
}

/// Set the core file size limit to 0 to prevent core dumps.
#[cfg(unix)]
fn set_core_file_size_limit_to_zero() {
    let rlim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };

    let ret_code = unsafe { libc::setrlimit(libc::RLIMIT_CORE, &rlim) };
    if ret_code != 0 {
        eprintln!(
            "ERROR: setrlimit(RLIMIT_CORE) failed: {}",
            std::io::Error::last_os_error()
        );
        std::process::exit(SET_RLIMIT_CORE_FAILED_EXIT_CODE);
    }
}

/// Clear LD_* environment variables on Linux/BSD.
#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd",
    target_os = "openbsd"
))]
fn clear_ld_env_vars() {
    let ld_keys: Vec<String> = std::env::vars()
        .filter_map(|(key, _)| {
            if key.starts_with("LD_") {
                Some(key)
            } else {
                None
            }
        })
        .collect();

    for key in ld_keys {
        // SAFETY: We're removing potentially dangerous environment variables
        // that could be used to inject malicious libraries
        unsafe {
            std::env::remove_var(key);
        }
    }
}

/// Clear DYLD_* environment variables on macOS.
#[cfg(target_os = "macos")]
fn clear_dyld_env_vars() {
    let dyld_keys: Vec<String> = std::env::vars()
        .filter_map(|(key, _)| {
            if key.starts_with("DYLD_") {
                Some(key)
            } else {
                None
            }
        })
        .collect();

    for key in dyld_keys {
        // SAFETY: We're removing potentially dangerous environment variables
        // that could be used to inject malicious libraries on macOS
        unsafe {
            std::env::remove_var(key);
        }
    }
}

/// Check if process hardening is available on this platform.
pub fn is_hardening_available() -> bool {
    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "windows"
    ))]
    {
        true
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "windows"
    )))]
    {
        false
    }
}

/// Get a description of the hardening measures applied on this platform.
pub fn hardening_description() -> &'static str {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        "Linux: PR_SET_DUMPABLE=0, RLIMIT_CORE=0, LD_* env vars cleared"
    }
    #[cfg(target_os = "macos")]
    {
        "macOS: PT_DENY_ATTACH, RLIMIT_CORE=0, DYLD_* env vars cleared"
    }
    #[cfg(any(target_os = "freebsd", target_os = "openbsd"))]
    {
        "BSD: RLIMIT_CORE=0, LD_* env vars cleared"
    }
    #[cfg(windows)]
    {
        "Windows: IsDebuggerPresent, DEP, ASLR, ExtensionPoints disabled, dangerous env vars cleared"
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "windows"
    )))]
    {
        "Unknown platform: no hardening available"
    }
}
