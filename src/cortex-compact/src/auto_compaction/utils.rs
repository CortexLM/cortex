//! Utility functions for auto-compaction.

use std::io;
use std::path::Path;
use std::time::SystemTime;

/// Get current Unix timestamp.
pub fn timestamp_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Get current timestamp as formatted string for filenames.
pub fn chrono_timestamp() -> String {
    let now = timestamp_now();
    format!("{}", now)
}

/// Estimate available disk space in bytes (platform-specific).
#[cfg(unix)]
#[allow(clippy::unnecessary_cast)] // Cast needed for cross-platform: macOS has u32, Linux has u64
pub fn available_disk_space(path: &Path) -> io::Result<u64> {
    use std::ffi::CString;
    use std::mem::MaybeUninit;

    let c_path = CString::new(path.to_string_lossy().as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

    unsafe {
        let mut statvfs = MaybeUninit::<libc::statvfs>::uninit();
        if libc::statvfs(c_path.as_ptr(), statvfs.as_mut_ptr()) == 0 {
            let statvfs = statvfs.assume_init();
            // Cast to u64 for cross-platform compatibility (macOS has u32 for f_bavail)
            Ok((statvfs.f_bavail as u64) * (statvfs.f_frsize as u64))
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

#[cfg(not(unix))]
pub fn available_disk_space(_path: &Path) -> io::Result<u64> {
    // Fallback for non-Unix platforms
    Ok(u64::MAX) // Assume infinite space
}
