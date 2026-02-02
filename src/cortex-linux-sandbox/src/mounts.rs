//! Mount namespace operations.
//!
//! Provides bind-mount operations to make paths read-only within a new
//! mount namespace. This is used to protect .git and .cortex directories
//! from modification even within writable roots.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use anyhow::{anyhow, Result};

/// Apply read-only bind mounts for the specified paths.
///
/// This function:
/// 1. Unshares user and mount namespaces (for non-root users)
/// 2. Makes all mounts private
/// 3. Bind-mounts each path onto itself and remounts read-only
/// 4. Drops capabilities acquired from the user namespace
pub fn apply_read_only_mounts(paths: &[PathBuf]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }

    // Check if running as root
    let is_root = unsafe { libc::geteuid() == 0 };

    if is_root {
        // Root can unshare mount namespace directly
        unshare_mount_namespace()?;
    } else {
        // Non-root needs user namespace to gain capabilities
        let original_euid = unsafe { libc::geteuid() };
        let original_egid = unsafe { libc::getegid() };
        unshare_user_and_mount_namespaces()?;
        write_user_namespace_maps(original_euid, original_egid)?;
    }

    // Make mounts private so remounting doesn't propagate outside namespace
    make_mounts_private()?;

    // Bind-mount each path read-only
    for path in paths {
        if path.exists() {
            bind_mount_read_only(path)?;
        }
    }

    // Drop capabilities for non-root
    if !is_root {
        drop_caps()?;
    }

    tracing::debug!("Read-only mounts applied for {} paths", paths.len());
    Ok(())
}

/// Unshare the mount namespace.
fn unshare_mount_namespace() -> Result<()> {
    let result = unsafe { libc::unshare(libc::CLONE_NEWNS) };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

/// Unshare user and mount namespaces.
fn unshare_user_and_mount_namespaces() -> Result<()> {
    let result = unsafe { libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNS) };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

/// Write uid/gid maps for the user namespace.
fn write_user_namespace_maps(uid: libc::uid_t, gid: libc::gid_t) -> Result<()> {
    // Deny setgroups first
    std::fs::write("/proc/self/setgroups", "deny\n")?;

    // Map the original uid/gid to root inside the namespace
    std::fs::write("/proc/self/uid_map", format!("0 {} 1\n", uid))?;
    std::fs::write("/proc/self/gid_map", format!("0 {} 1\n", gid))?;

    Ok(())
}

/// Make all mounts private.
fn make_mounts_private() -> Result<()> {
    let root = CString::new("/").map_err(|_| anyhow!("Invalid root path"))?;

    let result = unsafe {
        libc::mount(
            std::ptr::null(),
            root.as_ptr(),
            std::ptr::null(),
            libc::MS_REC | libc::MS_PRIVATE,
            std::ptr::null(),
        )
    };

    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

/// Bind-mount a path and remount it read-only.
fn bind_mount_read_only(path: &std::path::Path) -> Result<()> {
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| anyhow!("Path contains null byte: {}", path.display()))?;

    // Bind mount the path onto itself
    let bind_result = unsafe {
        libc::mount(
            c_path.as_ptr(),
            c_path.as_ptr(),
            std::ptr::null(),
            libc::MS_BIND,
            std::ptr::null(),
        )
    };

    if bind_result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    // Remount read-only
    let remount_result = unsafe {
        libc::mount(
            c_path.as_ptr(),
            c_path.as_ptr(),
            std::ptr::null(),
            libc::MS_BIND | libc::MS_REMOUNT | libc::MS_RDONLY,
            std::ptr::null(),
        )
    };

    if remount_result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    Ok(())
}

/// Drop all capabilities.
fn drop_caps() -> Result<()> {
    #[repr(C)]
    struct CapUserHeader {
        version: u32,
        pid: i32,
    }

    #[repr(C)]
    struct CapUserData {
        effective: u32,
        permitted: u32,
        inheritable: u32,
    }

    const LINUX_CAPABILITY_VERSION_3: u32 = 0x2008_0522;

    let mut header = CapUserHeader {
        version: LINUX_CAPABILITY_VERSION_3,
        pid: 0,
    };

    let data = [
        CapUserData {
            effective: 0,
            permitted: 0,
            inheritable: 0,
        },
        CapUserData {
            effective: 0,
            permitted: 0,
            inheritable: 0,
        },
    ];

    let result = unsafe { libc::syscall(libc::SYS_capset, &mut header, data.as_ptr()) };

    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    // Mount namespace tests need to be run as integration tests in subprocesses
    // because they affect the current process irreversibly
}
