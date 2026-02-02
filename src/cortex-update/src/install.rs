//! Platform-specific installation logic.

use std::path::{Path, PathBuf};
use tokio::process::Command;

use crate::error::{UpdateError, UpdateResult};
use crate::method::InstallMethod;

/// A downloaded and verified update ready for installation.
#[derive(Debug)]
pub struct DownloadedUpdate {
    /// Path to the downloaded archive
    pub archive_path: PathBuf,
    /// Version being installed
    pub version: String,
    /// SHA256 verified
    pub verified: bool,
}

impl DownloadedUpdate {
    /// Create a new downloaded update.
    pub fn new(archive_path: PathBuf, version: String) -> Self {
        Self {
            archive_path,
            version,
            verified: false,
        }
    }

    /// Mark as verified.
    pub fn mark_verified(&mut self) {
        self.verified = true;
    }
}

/// Installer for Cortex CLI updates.
pub struct Installer {
    method: InstallMethod,
}

impl Installer {
    /// Create a new installer.
    pub fn new(method: InstallMethod) -> Self {
        Self { method }
    }

    /// Install the update.
    pub async fn install(&self, download: &DownloadedUpdate) -> UpdateResult<()> {
        if !download.verified {
            return Err(UpdateError::InstallFailed {
                message: "Update not verified".to_string(),
            });
        }

        if self.method.uses_package_manager() {
            self.install_via_package_manager(&download.version).await
        } else {
            self.install_via_self_replace(download).await
        }
    }

    /// Install using the package manager.
    async fn install_via_package_manager(&self, version: &str) -> UpdateResult<()> {
        let args =
            self.method
                .update_command(version)
                .ok_or_else(|| UpdateError::UnsupportedMethod {
                    method: self.method.description().to_string(),
                })?;

        tracing::info!("Running: {}", args.join(" "));

        let status = Command::new(&args[0])
            .args(&args[1..])
            .status()
            .await
            .map_err(|e| UpdateError::InstallFailed {
                message: format!("Failed to run {}: {}", args[0], e),
            })?;

        if !status.success() {
            return Err(UpdateError::CommandFailed {
                command: args.join(" "),
                code: status.code().unwrap_or(-1),
            });
        }

        Ok(())
    }

    /// Install by replacing the current binary.
    async fn install_via_self_replace(&self, download: &DownloadedUpdate) -> UpdateResult<()> {
        // 1. Create temp directory for extraction
        let temp_dir = tempfile::tempdir().map_err(|_| UpdateError::TempDirFailed)?;

        // 2. Extract archive
        let binary_path = self
            .extract_archive(&download.archive_path, temp_dir.path())
            .await?;

        // 3. Replace current binary
        self.replace_binary(&binary_path).await?;

        Ok(())
    }

    /// Extract the archive and return path to the binary.
    async fn extract_archive(&self, archive: &Path, dest: &Path) -> UpdateResult<PathBuf> {
        let archive_str = archive.to_string_lossy().to_lowercase();

        if archive_str.ends_with(".tar.gz") || archive_str.ends_with(".tgz") {
            self.extract_tar_gz(archive, dest).await
        } else if archive_str.ends_with(".zip") {
            self.extract_zip(archive, dest).await
        } else {
            Err(UpdateError::ExtractionFailed {
                message: format!("Unknown archive format: {}", archive.display()),
            })
        }
    }

    /// Extract a tar.gz archive.
    async fn extract_tar_gz(&self, archive: &Path, dest: &Path) -> UpdateResult<PathBuf> {
        use flate2::read::GzDecoder;
        use std::fs::File;
        use tar::Archive;

        let file = File::open(archive)?;
        let gz = GzDecoder::new(file);
        let mut archive = Archive::new(gz);

        archive
            .unpack(dest)
            .map_err(|e| UpdateError::ExtractionFailed {
                message: e.to_string(),
            })?;

        self.find_binary(dest).await
    }

    /// Extract a zip archive.
    async fn extract_zip(&self, archive: &Path, dest: &Path) -> UpdateResult<PathBuf> {
        let file = std::fs::File::open(archive)?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| UpdateError::ExtractionFailed {
                message: e.to_string(),
            })?;

        archive
            .extract(dest)
            .map_err(|e| UpdateError::ExtractionFailed {
                message: e.to_string(),
            })?;

        self.find_binary(dest).await
    }

    /// Find the cortex binary in the extracted directory.
    async fn find_binary(&self, dir: &Path) -> UpdateResult<PathBuf> {
        #[cfg(windows)]
        let binary_name = "cortex.exe";
        #[cfg(not(windows))]
        let binary_name = "cortex";

        // Check direct in directory
        let direct = dir.join(binary_name);
        if direct.exists() {
            return Ok(direct);
        }

        // Check in bin/ subdirectory
        let in_bin = dir.join("bin").join(binary_name);
        if in_bin.exists() {
            return Ok(in_bin);
        }

        // Search recursively (one level)
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                let in_subdir = path.join(binary_name);
                if in_subdir.exists() {
                    return Ok(in_subdir);
                }
            }
        }

        Err(UpdateError::BinaryNotFound)
    }

    /// Replace the current binary with the new one.
    ///
    /// On Windows, if the binary is locked by antivirus or another process,
    /// this will attempt to schedule the replacement for the next reboot using
    /// MoveFileEx with MOVEFILE_DELAY_UNTIL_REBOOT.
    async fn replace_binary(&self, new_binary: &Path) -> UpdateResult<()> {
        // Use self-replace crate for atomic replacement
        match self_replace::self_replace(new_binary) {
            Ok(()) => Ok(()),
            Err(e) => {
                // On Windows, try delayed replacement if file is locked
                #[cfg(windows)]
                {
                    if let Some(os_error) = e.raw_os_error() {
                        // ERROR_SHARING_VIOLATION (32) or ERROR_LOCK_VIOLATION (33)
                        if os_error == 32 || os_error == 33 {
                            return self.schedule_delayed_replace(new_binary).await;
                        }
                    }
                }
                Err(UpdateError::ReplaceFailed {
                    message: e.to_string(),
                })
            }
        }
    }

    /// Schedule a delayed binary replacement on Windows.
    ///
    /// This uses MoveFileEx with MOVEFILE_DELAY_UNTIL_REBOOT to schedule
    /// the replacement for the next system restart.
    #[cfg(windows)]
    async fn schedule_delayed_replace(&self, new_binary: &Path) -> UpdateResult<()> {
        use std::os::windows::ffi::OsStrExt;

        let exe_path = std::env::current_exe()?;

        // Copy new binary to a staging location next to the current exe
        let staging_path = exe_path.with_extension("new.exe");
        std::fs::copy(new_binary, &staging_path).map_err(|e| UpdateError::ReplaceFailed {
            message: format!("Failed to copy new binary to staging: {}", e),
        })?;

        // Convert paths to wide strings for Windows API
        let old_path: Vec<u16> = staging_path
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let new_path: Vec<u16> = exe_path.as_os_str().encode_wide().chain(Some(0)).collect();

        // MOVEFILE_DELAY_UNTIL_REBOOT | MOVEFILE_REPLACE_EXISTING
        const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
        const MOVEFILE_DELAY_UNTIL_REBOOT: u32 = 0x4;

        let result = unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                old_path.as_ptr(),
                new_path.as_ptr(),
                MOVEFILE_DELAY_UNTIL_REBOOT | MOVEFILE_REPLACE_EXISTING,
            )
        };

        if result == 0 {
            let error = std::io::Error::last_os_error();
            return Err(UpdateError::ReplaceFailed {
                message: format!(
                    "Failed to schedule delayed replacement: {}. Please restart your computer and run the upgrade again.",
                    error
                ),
            });
        }

        Err(UpdateError::RequiresRestart {
            message: "Update downloaded successfully. The binary is currently locked by another process. Please restart your computer to complete the upgrade.".to_string(),
        })
    }
}

/// Check if we have permission to write to the installation directory.
pub fn check_write_permission() -> UpdateResult<()> {
    let exe_path = std::env::current_exe()?;
    let parent = exe_path
        .parent()
        .ok_or_else(|| UpdateError::PermissionDenied {
            path: exe_path.clone(),
        })?;

    // Try to create a temp file to check write permission
    let test_path = parent.join(".cortex_update_test");
    match std::fs::write(&test_path, b"test") {
        Ok(_) => {
            let _ = std::fs::remove_file(&test_path);
            Ok(())
        }
        Err(_) => Err(UpdateError::PermissionDenied {
            path: parent.to_path_buf(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_downloaded_update() {
        // Use a cross-platform temp path
        let temp_path = std::env::temp_dir().join("test.tar.gz");
        let mut download = DownloadedUpdate::new(temp_path, "0.2.0".to_string());

        assert!(!download.verified);
        download.mark_verified();
        assert!(download.verified);
    }
}
