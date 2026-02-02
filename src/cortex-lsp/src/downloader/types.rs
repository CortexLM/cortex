//! Types and constants for the LSP server downloader.

/// Default directory for storing LSP servers.
pub const LSP_SERVERS_DIR: &str = ".cortex/lsp-servers";

/// Progress callback type for download progress reporting.
pub type ProgressCallback = Box<dyn Fn(u64, u64) + Send + Sync>;

/// Installation method for LSP servers.
#[derive(Debug, Clone)]
pub enum InstallMethod {
    /// Download from GitHub releases.
    GitHubRelease {
        /// GitHub owner/repo.
        repo: String,
        /// Asset name pattern for the release.
        asset_pattern: String,
        /// Whether the asset is an archive (zip/tar.gz).
        is_archive: bool,
        /// Path within archive to the binary (if is_archive is true).
        archive_binary_path: Option<String>,
    },
    /// Install via npm.
    Npm {
        /// npm package name.
        package: String,
        /// Binary name (command to run after install).
        binary_name: String,
    },
    /// Download from a custom URL pattern.
    CustomUrl {
        /// URL pattern (supports {version}, {os}, {arch}).
        url_pattern: String,
        /// Whether the download is an archive.
        is_archive: bool,
        /// Path within archive to the binary.
        archive_binary_path: Option<String>,
    },
}

/// Downloadable LSP server definition.
#[derive(Debug, Clone)]
pub struct DownloadableServer {
    /// Server identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// GitHub owner/repo (kept for backward compatibility).
    pub github_repo: String,
    /// Binary name pattern (supports {version}, {os}, {arch}).
    pub binary_pattern: String,
    /// Asset name pattern for the release.
    pub asset_pattern: String,
    /// Whether the asset is an archive (zip/tar.gz).
    pub is_archive: bool,
    /// Path within archive to the binary (if is_archive is true).
    pub archive_binary_path: Option<String>,
    /// Installation method (optional, defaults to GitHub release).
    pub install_method: Option<InstallMethod>,
}
