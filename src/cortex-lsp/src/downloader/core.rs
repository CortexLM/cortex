//! Core LSP downloader implementation.

#![allow(
    clippy::collapsible_else_if,
    clippy::redundant_closure,
    clippy::useless_format,
    clippy::double_ended_iterator_last,
    clippy::io_other_error
)]

use crate::{LspError, Result};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::{debug, info};

use super::archive::{extract_tar_gz, extract_tar_xz, extract_zip, find_binary_recursive};
use super::http::{create_http_client, GitHubRelease};
use super::types::{DownloadableServer, InstallMethod, ProgressCallback, LSP_SERVERS_DIR};

/// LSP server downloader.
pub struct LspDownloader {
    /// Base directory for storing servers.
    base_dir: PathBuf,
    /// HTTP client for downloads.
    client: reqwest::Client,
}

impl LspDownloader {
    /// Create a new downloader.
    pub fn new() -> Result<Self> {
        let home = dirs::home_dir()
            .ok_or_else(|| LspError::StartFailed("Could not determine home directory".into()))?;
        let base_dir = home.join(LSP_SERVERS_DIR);

        let client = create_http_client()
            .map_err(|e| LspError::Communication(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self { base_dir, client })
    }

    /// Create a downloader with a custom base directory.
    pub fn with_base_dir(base_dir: impl Into<PathBuf>) -> Result<Self> {
        let base_dir = base_dir.into();
        let client = create_http_client()
            .map_err(|e| LspError::Communication(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self { base_dir, client })
    }

    /// Get the base directory for LSP servers.
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Check if a server is already downloaded.
    pub async fn is_installed(&self, server_id: &str) -> bool {
        let server_dir = self.base_dir.join(server_id);
        server_dir.exists()
    }

    /// Get the path to a server's binary.
    pub fn get_binary_path(&self, server: &DownloadableServer) -> PathBuf {
        let binary_name = self.resolve_pattern(&server.binary_pattern, "latest");
        self.base_dir.join(&server.id).join(binary_name)
    }

    /// Download a server if not already installed.
    pub async fn ensure_installed(&self, server: &DownloadableServer) -> Result<PathBuf> {
        self.ensure_installed_with_progress(server, None).await
    }

    /// Download a server if not already installed with progress reporting.
    pub async fn ensure_installed_with_progress(
        &self,
        server: &DownloadableServer,
        progress: Option<ProgressCallback>,
    ) -> Result<PathBuf> {
        let binary_path = self.get_binary_path(server);

        if binary_path.exists() {
            debug!(
                "Server {} already installed at {:?}",
                server.id, binary_path
            );
            return Ok(binary_path);
        }

        // Check installation method
        if let Some(ref method) = server.install_method {
            match method {
                InstallMethod::Npm {
                    package,
                    binary_name,
                } => {
                    info!("Installing {} via npm...", server.name);
                    return self.install_npm_server(package, binary_name).await;
                }
                InstallMethod::CustomUrl {
                    url_pattern,
                    is_archive,
                    archive_binary_path,
                } => {
                    info!("Downloading {} from custom URL...", server.name);
                    return self
                        .download_from_custom_url(
                            server,
                            url_pattern,
                            *is_archive,
                            archive_binary_path.clone(),
                            progress,
                        )
                        .await;
                }
                InstallMethod::GitHubRelease { .. } => {
                    // Fall through to GitHub release handling
                }
            }
        }

        info!("Downloading {} from {}...", server.name, server.github_repo);
        self.download_server_with_progress(server, progress).await
    }

    /// Download a server from GitHub releases.
    pub async fn download_server(&self, server: &DownloadableServer) -> Result<PathBuf> {
        self.download_server_with_progress(server, None).await
    }

    /// Download a server from GitHub releases with progress reporting.
    pub async fn download_server_with_progress(
        &self,
        server: &DownloadableServer,
        progress: Option<ProgressCallback>,
    ) -> Result<PathBuf> {
        // Ensure base directory exists
        let server_dir = self.base_dir.join(&server.id);
        fs::create_dir_all(&server_dir)
            .await
            .map_err(LspError::Io)?;

        // Fetch latest release info
        let release = self.fetch_latest_release(&server.github_repo).await?;
        let version = release.tag_name.trim_start_matches('v').to_string();

        // Find matching asset
        let asset_pattern = self.resolve_pattern(&server.asset_pattern, &version);
        let asset = release
            .assets
            .iter()
            .find(|a| Self::matches_pattern(&a.name, &asset_pattern))
            .ok_or_else(|| {
                LspError::ServerNotFound(format!(
                    "No matching asset found for pattern: {} (available: {:?})",
                    asset_pattern,
                    release.assets.iter().map(|a| &a.name).collect::<Vec<_>>()
                ))
            })?;

        info!("Downloading asset: {}", asset.name);

        // Download the asset
        let download_path = server_dir.join(&asset.name);
        self.download_file_with_progress(&asset.browser_download_url, &download_path, progress)
            .await?;

        // Extract if it's an archive
        let binary_path = if server.is_archive {
            self.extract_archive(&download_path, &server_dir, server)
                .await?
        } else {
            // Just rename/move the binary
            let binary_name = self.resolve_pattern(&server.binary_pattern, &version);
            let final_path = server_dir.join(&binary_name);
            fs::rename(&download_path, &final_path)
                .await
                .map_err(LspError::Io)?;
            final_path
        };

        // Make executable on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&binary_path)
                .await
                .map_err(LspError::Io)?
                .permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&binary_path, perms)
                .await
                .map_err(LspError::Io)?;
        }

        info!(
            "Successfully installed {} at {:?}",
            server.name, binary_path
        );
        Ok(binary_path)
    }

    /// Install a server via npm.
    async fn install_npm_server(&self, package: &str, binary_name: &str) -> Result<PathBuf> {
        // Check if npm is available
        let npm_available = Command::new("npm")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false);

        if !npm_available {
            return Err(LspError::StartFailed(
                "npm is not installed. Please install Node.js and npm to use this language server."
                    .into(),
            ));
        }

        info!("Installing {} via npm (this may take a moment)...", package);

        // Install globally
        let status = Command::new("npm")
            .args(["install", "-g", package])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .status()
            .await
            .map_err(|e| LspError::StartFailed(format!("Failed to run npm install: {}", e)))?;

        if !status.success() {
            return Err(LspError::StartFailed(format!(
                "npm install -g {} failed with exit code: {:?}",
                package,
                status.code()
            )));
        }

        // Find the installed binary
        let which_cmd = if cfg!(windows) { "where" } else { "which" };
        let output = Command::new(which_cmd)
            .arg(binary_name)
            .output()
            .await
            .map_err(|e| {
                LspError::StartFailed(format!("Failed to locate installed binary: {}", e))
            })?;

        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string();

            if !path.is_empty() {
                info!("Successfully installed {} at {}", package, path);
                return Ok(PathBuf::from(path));
            }
        }

        // Fallback: try common npm global paths
        let binary_path = self.find_npm_binary(binary_name).await?;
        info!("Successfully installed {} at {:?}", package, binary_path);
        Ok(binary_path)
    }

    /// Find an npm binary in common locations.
    async fn find_npm_binary(&self, binary_name: &str) -> Result<PathBuf> {
        // Try npm bin -g to get global bin directory
        let output = Command::new("npm").args(["bin", "-g"]).output().await.ok();

        if let Some(output) = output {
            if output.status.success() {
                let bin_dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let binary_path = PathBuf::from(&bin_dir).join(if cfg!(windows) {
                    format!("{}.cmd", binary_name)
                } else {
                    binary_name.to_string()
                });

                if binary_path.exists() {
                    return Ok(binary_path);
                }
            }
        }

        Err(LspError::ServerNotFound(format!(
            "Could not find {} after npm installation. Try running 'npm install -g {}' manually.",
            binary_name, binary_name
        )))
    }

    /// Download from a custom URL.
    async fn download_from_custom_url(
        &self,
        server: &DownloadableServer,
        url_pattern: &str,
        is_archive: bool,
        archive_binary_path: Option<String>,
        progress: Option<ProgressCallback>,
    ) -> Result<PathBuf> {
        let server_dir = self.base_dir.join(&server.id);
        fs::create_dir_all(&server_dir)
            .await
            .map_err(LspError::Io)?;

        let url = self.resolve_pattern(url_pattern, "latest");
        let file_name = url.split('/').last().unwrap_or("download");
        let download_path = server_dir.join(file_name);

        info!("Downloading from: {}", url);
        self.download_file_with_progress(&url, &download_path, progress)
            .await?;

        let binary_path = if is_archive {
            let temp_server = DownloadableServer {
                id: server.id.clone(),
                name: server.name.clone(),
                github_repo: server.github_repo.clone(),
                binary_pattern: server.binary_pattern.clone(),
                asset_pattern: server.asset_pattern.clone(),
                is_archive: true,
                archive_binary_path,
                install_method: None,
            };
            self.extract_archive(&download_path, &server_dir, &temp_server)
                .await?
        } else {
            let binary_name = self.resolve_pattern(&server.binary_pattern, "latest");
            let final_path = server_dir.join(&binary_name);
            fs::rename(&download_path, &final_path)
                .await
                .map_err(LspError::Io)?;
            final_path
        };

        // Make executable on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&binary_path)
                .await
                .map_err(LspError::Io)?
                .permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&binary_path, perms)
                .await
                .map_err(LspError::Io)?;
        }

        info!(
            "Successfully installed {} at {:?}",
            server.name, binary_path
        );
        Ok(binary_path)
    }

    /// Fetch the latest release from GitHub.
    async fn fetch_latest_release(&self, repo: &str) -> Result<GitHubRelease> {
        let url = format!("https://api.github.com/repos/{}/releases/latest", repo);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| LspError::Communication(format!("Failed to fetch release: {}", e)))?;

        if !response.status().is_success() {
            return Err(LspError::Communication(format!(
                "GitHub API returned status: {}",
                response.status()
            )));
        }

        response
            .json()
            .await
            .map_err(|e| LspError::Communication(format!("Failed to parse release JSON: {}", e)))
    }

    /// Download a file from a URL.
    #[allow(dead_code)]
    async fn download_file(&self, url: &str, path: &Path) -> Result<()> {
        self.download_file_with_progress(url, path, None).await
    }

    /// Download a file from a URL with progress reporting.
    async fn download_file_with_progress(
        &self,
        url: &str,
        path: &Path,
        progress: Option<ProgressCallback>,
    ) -> Result<()> {
        use futures::StreamExt;

        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| LspError::Communication(format!("Failed to download: {}", e)))?;

        if !response.status().is_success() {
            return Err(LspError::Communication(format!(
                "Download failed with status: {}",
                response.status()
            )));
        }

        let total_size = response.content_length().unwrap_or(0);
        let mut downloaded: u64 = 0;

        let mut file = fs::File::create(path).await.map_err(LspError::Io)?;

        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk
                .map_err(|e| LspError::Communication(format!("Failed to read chunk: {}", e)))?;

            file.write_all(&chunk).await.map_err(LspError::Io)?;

            downloaded += chunk.len() as u64;

            if let Some(ref cb) = progress {
                cb(downloaded, total_size);
            }
        }

        // Log download size for large files
        if total_size > 10 * 1024 * 1024 {
            info!("Downloaded {:.1} MB", total_size as f64 / (1024.0 * 1024.0));
        }

        Ok(())
    }

    /// Extract an archive and return the path to the binary.
    async fn extract_archive(
        &self,
        archive_path: &Path,
        dest_dir: &Path,
        server: &DownloadableServer,
    ) -> Result<PathBuf> {
        let archive_name = archive_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        if archive_name.ends_with(".zip") {
            extract_zip(archive_path, dest_dir).await?;
        } else if archive_name.ends_with(".tar.gz") || archive_name.ends_with(".tgz") {
            extract_tar_gz(archive_path, dest_dir).await?;
        } else if archive_name.ends_with(".tar.xz") || archive_name.ends_with(".txz") {
            extract_tar_xz(archive_path, dest_dir).await?;
        } else {
            return Err(LspError::Communication(format!(
                "Unsupported archive format: {}",
                archive_name
            )));
        }

        // Clean up the archive
        let _ = fs::remove_file(archive_path).await;

        // Find the binary
        if let Some(ref binary_subpath) = server.archive_binary_path {
            let binary_path = dest_dir.join(binary_subpath);
            if binary_path.exists() {
                return Ok(binary_path);
            }
        }

        // Try to find the binary by pattern
        let binary_name = self.resolve_pattern(&server.binary_pattern, "latest");
        let binary_path = dest_dir.join(&binary_name);
        if binary_path.exists() {
            return Ok(binary_path);
        }

        // Search for the binary in subdirectories
        if let Some(path) = find_binary_recursive(dest_dir, &binary_name).await? {
            return Ok(path);
        }

        Err(LspError::ServerNotFound(format!(
            "Could not find binary {} in extracted archive",
            binary_name
        )))
    }

    /// Resolve a pattern with OS/arch/version placeholders.
    pub fn resolve_pattern(&self, pattern: &str, version: &str) -> String {
        let os = if cfg!(target_os = "windows") {
            "windows"
        } else if cfg!(target_os = "macos") {
            "darwin"
        } else {
            "linux"
        };

        let arch = if cfg!(target_arch = "x86_64") {
            "x86_64"
        } else if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else {
            "x86_64"
        };

        let ext = if cfg!(target_os = "windows") {
            ".exe"
        } else {
            ""
        };

        pattern
            .replace("{version}", version)
            .replace("{os}", os)
            .replace("{arch}", arch)
            .replace("{ext}", ext)
    }

    /// Check if an asset name matches a pattern (supports wildcards).
    pub fn matches_pattern(name: &str, pattern: &str) -> bool {
        if pattern.contains('*') {
            let parts: Vec<&str> = pattern.split('*').collect();
            let mut remaining = name;

            for (i, part) in parts.iter().enumerate() {
                if part.is_empty() {
                    continue;
                }

                if i == 0 {
                    if !remaining.starts_with(part) {
                        return false;
                    }
                    remaining = &remaining[part.len()..];
                } else if i == parts.len() - 1 {
                    if !remaining.ends_with(part) {
                        return false;
                    }
                } else {
                    if let Some(pos) = remaining.find(part) {
                        remaining = &remaining[pos + part.len()..];
                    } else {
                        return false;
                    }
                }
            }
            true
        } else {
            name == pattern
        }
    }
}

impl Default for LspDownloader {
    fn default() -> Self {
        Self::new().expect("Failed to create LspDownloader")
    }
}
