//! WSL (Windows Subsystem for Linux) path handling.
//!
//! Utilities for converting between Windows and WSL paths.

use std::ffi::OsStr;

/// Check if running under WSL.
pub fn is_wsl() -> bool {
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("WSL_DISTRO_NAME").is_some() {
            return true;
        }
        match std::fs::read_to_string("/proc/version") {
            Ok(version) => version.to_lowercase().contains("microsoft"),
            Err(_) => false,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Convert a Windows absolute path to a WSL mount path.
///
/// Converts paths like `C:\foo\bar` or `C:/foo/bar` to `/mnt/c/foo/bar`.
/// Returns `None` if the input doesn't look like a Windows drive path.
pub fn win_path_to_wsl(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    if bytes.len() < 3
        || bytes[1] != b':'
        || !(bytes[2] == b'\\' || bytes[2] == b'/')
        || !bytes[0].is_ascii_alphabetic()
    {
        return None;
    }
    let drive = (bytes[0] as char).to_ascii_lowercase();
    let tail = path[3..].replace('\\', "/");
    if tail.is_empty() {
        return Some(format!("/mnt/{drive}"));
    }
    Some(format!("/mnt/{drive}/{tail}"))
}

/// Convert a WSL mount path to a Windows path.
///
/// Converts paths like `/mnt/c/foo/bar` to `C:/foo/bar`.
/// Returns `None` if the input doesn't look like a WSL mount path.
pub fn wsl_path_to_win(path: &str) -> Option<String> {
    if !path.starts_with("/mnt/") {
        return None;
    }

    let rest = &path[5..];
    if rest.is_empty() {
        return None;
    }

    let bytes = rest.as_bytes();
    if !bytes[0].is_ascii_alphabetic() {
        return None;
    }

    let drive = (bytes[0] as char).to_ascii_uppercase();

    if rest.len() == 1 {
        return Some(format!("{drive}:/"));
    }

    if bytes[1] != b'/' {
        return None;
    }

    let tail = &rest[2..];
    Some(format!("{drive}:/{tail}"))
}

/// If under WSL and given a Windows-style path, return the equivalent WSL path.
/// Otherwise returns the input unchanged.
pub fn normalize_for_wsl<P: AsRef<OsStr>>(path: P) -> String {
    let value = path.as_ref().to_string_lossy().to_string();
    if !is_wsl() {
        return value;
    }
    if let Some(mapped) = win_path_to_wsl(&value) {
        return mapped;
    }
    value
}

/// If under WSL and given a WSL mount path, return the equivalent Windows path.
/// Otherwise returns the input unchanged.
pub fn normalize_for_windows<P: AsRef<OsStr>>(path: P) -> String {
    let value = path.as_ref().to_string_lossy().to_string();
    if !is_wsl() {
        return value;
    }
    if let Some(mapped) = wsl_path_to_win(&value) {
        return mapped;
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn win_to_wsl_basic() {
        assert_eq!(
            win_path_to_wsl(r"C:\Temp\file.zip").as_deref(),
            Some("/mnt/c/Temp/file.zip")
        );
        assert_eq!(
            win_path_to_wsl("D:/Work/file.tgz").as_deref(),
            Some("/mnt/d/Work/file.tgz")
        );
        assert!(win_path_to_wsl("/home/user/file").is_none());
    }

    #[test]
    fn wsl_to_win_basic() {
        assert_eq!(
            wsl_path_to_win("/mnt/c/Temp/file.zip").as_deref(),
            Some("C:/Temp/file.zip")
        );
        assert_eq!(
            wsl_path_to_win("/mnt/d/Work/file.tgz").as_deref(),
            Some("D:/Work/file.tgz")
        );
        assert!(wsl_path_to_win("/home/user/file").is_none());
    }

    #[test]
    fn normalize_is_noop_on_unix_paths() {
        assert_eq!(normalize_for_wsl("/home/u/x"), "/home/u/x");
    }

    #[test]
    fn drive_root_paths() {
        // win_path_to_wsl returns without trailing slash for root paths
        assert_eq!(win_path_to_wsl("C:/").as_deref(), Some("/mnt/c"));
        assert_eq!(wsl_path_to_win("/mnt/c").as_deref(), Some("C:/"));
    }

    #[test]
    fn win_to_wsl_all_drive_letters() {
        // Test lowercase drive letters
        assert_eq!(win_path_to_wsl("a:/test").as_deref(), Some("/mnt/a/test"));
        assert_eq!(win_path_to_wsl("z:/test").as_deref(), Some("/mnt/z/test"));
        // Test uppercase drive letters (should be normalized to lowercase)
        assert_eq!(win_path_to_wsl("A:/test").as_deref(), Some("/mnt/a/test"));
        assert_eq!(win_path_to_wsl("Z:/test").as_deref(), Some("/mnt/z/test"));
    }

    #[test]
    fn win_to_wsl_backslash_conversion() {
        // Mixed slashes should work
        assert_eq!(
            win_path_to_wsl(r"C:\Users\test/Documents\file.txt").as_deref(),
            Some("/mnt/c/Users/test/Documents/file.txt")
        );
        // All backslashes
        assert_eq!(
            win_path_to_wsl(r"C:\Users\test\Documents").as_deref(),
            Some("/mnt/c/Users/test/Documents")
        );
    }

    #[test]
    fn win_to_wsl_invalid_paths() {
        // Too short
        assert!(win_path_to_wsl("").is_none());
        assert!(win_path_to_wsl("C").is_none());
        assert!(win_path_to_wsl("C:").is_none());
        // Missing colon
        assert!(win_path_to_wsl("C/test").is_none());
        // Invalid separator
        assert!(win_path_to_wsl("C:test").is_none());
        // Non-alphabetic drive letter
        assert!(win_path_to_wsl("1:/test").is_none());
        assert!(win_path_to_wsl("_:/test").is_none());
        // Unix-style paths
        assert!(win_path_to_wsl("/mnt/c/test").is_none());
        assert!(win_path_to_wsl("./relative/path").is_none());
    }

    #[test]
    fn win_to_wsl_drive_root_only() {
        // Drive root with backslash
        assert_eq!(win_path_to_wsl(r"C:\").as_deref(), Some("/mnt/c"));
        // Drive root with forward slash
        assert_eq!(win_path_to_wsl("D:/").as_deref(), Some("/mnt/d"));
    }

    #[test]
    fn wsl_to_win_all_drive_letters() {
        // Test lowercase drive letters (should be normalized to uppercase)
        assert_eq!(wsl_path_to_win("/mnt/a/test").as_deref(), Some("A:/test"));
        assert_eq!(wsl_path_to_win("/mnt/z/test").as_deref(), Some("Z:/test"));
    }

    #[test]
    fn wsl_to_win_invalid_paths() {
        // Not starting with /mnt/
        assert!(wsl_path_to_win("/home/user/file").is_none());
        assert!(wsl_path_to_win("/var/log/file").is_none());
        assert!(wsl_path_to_win("mnt/c/test").is_none());
        // Just /mnt/ without drive letter
        assert!(wsl_path_to_win("/mnt/").is_none());
        // Non-alphabetic after /mnt/
        assert!(wsl_path_to_win("/mnt/1/test").is_none());
        assert!(wsl_path_to_win("/mnt/_/test").is_none());
        // Drive letter not followed by slash
        assert!(wsl_path_to_win("/mnt/ctest").is_none());
        // Empty string
        assert!(wsl_path_to_win("").is_none());
    }

    #[test]
    fn wsl_to_win_drive_root_only() {
        // Single drive letter without trailing content
        assert_eq!(wsl_path_to_win("/mnt/c").as_deref(), Some("C:/"));
        assert_eq!(wsl_path_to_win("/mnt/d").as_deref(), Some("D:/"));
    }

    #[test]
    fn wsl_to_win_deep_paths() {
        assert_eq!(
            wsl_path_to_win("/mnt/c/Users/test/Documents/folder/subfolder/file.txt").as_deref(),
            Some("C:/Users/test/Documents/folder/subfolder/file.txt")
        );
    }

    #[test]
    fn win_to_wsl_deep_paths() {
        assert_eq!(
            win_path_to_wsl(r"C:\Users\test\Documents\folder\subfolder\file.txt").as_deref(),
            Some("/mnt/c/Users/test/Documents/folder/subfolder/file.txt")
        );
    }

    #[test]
    fn normalize_for_wsl_with_osstr() {
        // Test that normalize_for_wsl accepts OsStr-compatible types
        let path = std::path::PathBuf::from("/home/user/file");
        let result = normalize_for_wsl(&path);
        assert_eq!(result, "/home/user/file");

        let str_path = "/some/path";
        let result = normalize_for_wsl(str_path);
        assert_eq!(result, "/some/path");
    }

    #[test]
    fn normalize_for_windows_with_osstr() {
        // Test that normalize_for_windows accepts OsStr-compatible types
        let path = std::path::PathBuf::from("/home/user/file");
        let result = normalize_for_windows(&path);
        assert_eq!(result, "/home/user/file");

        let str_path = "/some/path";
        let result = normalize_for_windows(str_path);
        assert_eq!(result, "/some/path");
    }

    #[test]
    fn path_conversion_roundtrip() {
        // Test that converting from Windows to WSL and back gives equivalent result
        let win_path = "C:/Users/test/file.txt";
        let wsl_path = win_path_to_wsl(win_path).expect("should convert to WSL");
        let back_to_win = wsl_path_to_win(&wsl_path).expect("should convert back to Windows");
        assert_eq!(back_to_win, win_path);

        // Same with backslashes (converted to forward slashes)
        let win_backslash = r"D:\Projects\Code\main.rs";
        let wsl_converted = win_path_to_wsl(win_backslash).expect("should convert");
        let back = wsl_path_to_win(&wsl_converted).expect("should convert back");
        assert_eq!(back, "D:/Projects/Code/main.rs");
    }

    #[test]
    fn wsl_path_to_win_roundtrip() {
        // Test that converting from WSL to Windows and back gives same result
        let wsl_path = "/mnt/e/Documents/notes.md";
        let win_path = wsl_path_to_win(wsl_path).expect("should convert to Windows");
        let back_to_wsl = win_path_to_wsl(&win_path).expect("should convert back to WSL");
        assert_eq!(back_to_wsl, wsl_path);
    }

    #[test]
    fn is_wsl_returns_bool() {
        // Simply verify is_wsl returns a boolean (actual value depends on environment)
        let result = is_wsl();
        // Just verify the function works
        let _ = result;
    }

    #[test]
    fn win_path_with_spaces() {
        assert_eq!(
            win_path_to_wsl(r"C:\Program Files\App\file.exe").as_deref(),
            Some("/mnt/c/Program Files/App/file.exe")
        );
        assert_eq!(
            win_path_to_wsl("D:/My Documents/file with spaces.txt").as_deref(),
            Some("/mnt/d/My Documents/file with spaces.txt")
        );
    }

    #[test]
    fn wsl_path_with_spaces() {
        assert_eq!(
            wsl_path_to_win("/mnt/c/Program Files/App/file.exe").as_deref(),
            Some("C:/Program Files/App/file.exe")
        );
    }

    #[test]
    fn paths_with_special_characters() {
        // Test paths with dots, dashes, underscores
        assert_eq!(
            win_path_to_wsl(r"C:\my-folder_v2.0\file.tar.gz").as_deref(),
            Some("/mnt/c/my-folder_v2.0/file.tar.gz")
        );
        assert_eq!(
            wsl_path_to_win("/mnt/c/my-folder_v2.0/file.tar.gz").as_deref(),
            Some("C:/my-folder_v2.0/file.tar.gz")
        );
    }
}
