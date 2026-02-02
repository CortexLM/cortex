//! HTTP client and GitHub API types for the downloader.

use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

/// User-Agent for HTTP requests
pub const USER_AGENT: &str = concat!("cortex-cli/", env!("CARGO_PKG_VERSION"));

/// Default timeout for HTTP requests (30 seconds)
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Create an HTTP client with proper configuration
pub fn create_http_client() -> Result<Client, String> {
    Client::builder()
        .user_agent(USER_AGENT)
        .timeout(DEFAULT_TIMEOUT)
        .tcp_nodelay(true)
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))
}

/// GitHub release API response.
#[derive(Debug, Deserialize)]
pub struct GitHubRelease {
    pub tag_name: String,
    pub assets: Vec<GitHubAsset>,
}

/// GitHub release asset.
#[derive(Debug, Deserialize)]
pub struct GitHubAsset {
    pub name: String,
    pub browser_download_url: String,
}
