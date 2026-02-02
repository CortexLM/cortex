//! Upgrade command - check for and install updates.
//!
//! Uses the Cortex Software Distribution API at software.cortex.foundation
//! to check for updates and download new versions.

use anyhow::{Context, Result};
use clap::Parser;
use cortex_engine::create_default_client;
use std::io::{Write, stdout};

use cortex_update::{
    ReleaseChannel, SOFTWARE_URL, UpdateConfig, UpdateInfo, UpdateManager, UpdateOutcome,
};

/// Current CLI version from this binary's Cargo.toml
const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Upgrade CLI.
#[derive(Debug, Parser)]
pub struct UpgradeCli {
    /// Target version to upgrade to (e.g., "1.2.0")
    /// If not specified, upgrades to the latest version.
    #[arg(value_name = "VERSION")]
    pub version: Option<String>,

    /// Only check for updates without installing
    #[arg(long, short = 'c')]
    pub check: bool,

    /// Show changelog for the target version
    #[arg(long)]
    pub changelog: bool,

    /// Force upgrade even if already on the target version
    #[arg(long, short = 'f')]
    pub force: bool,

    /// Skip confirmation prompts
    #[arg(long, short = 'y')]
    pub yes: bool,

    /// Release channel to use (stable, beta, nightly)
    #[arg(long, default_value = "stable")]
    pub channel: String,

    /// Include prerelease versions (shorthand for --channel beta).
    /// When specified, allows installing beta/prerelease versions.
    #[arg(long, conflicts_with = "channel")]
    pub pre: bool,

    /// Use custom software distribution URL
    #[arg(long, hide = true)]
    pub url: Option<String>,
}

impl UpgradeCli {
    /// Run the upgrade command.
    pub async fn run(self) -> Result<()> {
        println!("Cortex CLI Upgrade");
        println!("{}", "=".repeat(40));
        println!("Current version: v{}", CLI_VERSION);
        println!(
            "Update server: {}",
            self.url.as_deref().unwrap_or(SOFTWARE_URL)
        );

        // Parse channel (--pre is shorthand for --channel beta)
        let channel = if self.pre {
            ReleaseChannel::Beta
        } else {
            match self.channel.as_str() {
                "stable" => ReleaseChannel::Stable,
                "beta" => ReleaseChannel::Beta,
                "nightly" => ReleaseChannel::Nightly,
                _ => {
                    eprintln!(
                        "Invalid channel: {}. Use: stable, beta, or nightly",
                        self.channel
                    );
                    return Ok(());
                }
            }
        };

        // Create config
        let mut config = UpdateConfig::load();
        config.channel = channel;
        if let Some(url) = &self.url {
            config.custom_url = Some(url.clone());
        }

        // Create update manager
        let manager =
            UpdateManager::with_config(config).context("Failed to initialize update manager")?;

        // Check for specific version or latest
        let update_info = if let Some(ref version) = self.version {
            println!("\nChecking version {}...", version);
            match check_specific_version(&manager, version).await {
                Ok(info) => Some(info),
                Err(e) => {
                    // Check if user asked for current version (Issue #1968)
                    let normalized_version = version.trim_start_matches('v');
                    let normalized_current = CLI_VERSION.trim_start_matches('v');
                    if normalized_version == normalized_current {
                        println!("\n✓ Already on version v{}. No action needed.", CLI_VERSION);
                        return Ok(());
                    }
                    eprintln!("Error: {}", e);
                    return Ok(());
                }
            }
        } else {
            println!("\nChecking for updates ({} channel)...", self.channel);
            match manager.check_update_forced().await {
                Ok(Some(info)) => {
                    // Show version comparison (Issue #1965)
                    println!(
                        "  Current: v{}  |  Latest: v{}",
                        info.current_version, info.latest_version
                    );
                    Some(info)
                }
                Ok(None) => {
                    println!(
                        "\n✓ You are already on the latest version (v{})",
                        CLI_VERSION
                    );
                    return Ok(());
                }
                Err(e) => {
                    eprintln!("Failed to check for updates: {}", e);
                    eprintln!("\nTip: Check {} manually.", SOFTWARE_URL);
                    return Ok(());
                }
            }
        };

        let Some(info) = update_info else {
            return Ok(());
        };

        // Display update info
        // Check if versions are the same (or if current is already newer for downgrades)
        let version_cmp = semver_compare(&info.current_version, &info.latest_version);

        if version_cmp == 0 && !self.force {
            println!(
                "\n✓ Already on v{} ({} channel). No upgrade needed.",
                info.latest_version, self.channel
            );
            println!("  Use --force to reinstall the same version.");
            return Ok(());
        }

        let is_upgrade = version_cmp < 0;
        if is_upgrade {
            println!(
                "\n→ Update available: v{} → v{} ({} channel)",
                info.current_version, info.latest_version, self.channel
            );
        } else if version_cmp == 0 {
            // Force reinstall case
            println!(
                "\n⟳ Reinstalling v{} ({} channel) (--force)",
                info.latest_version, self.channel
            );
        } else {
            println!(
                "\n↓ Downgrade requested: v{} → v{} ({} channel)",
                info.current_version, info.latest_version, self.channel
            );
        }

        // Show release notes if available
        if let Some(notes) = &info.release_notes {
            println!("\nRelease notes: {}", notes);
        }

        // Show changelog if requested
        if self.changelog {
            if let Some(url) = &info.changelog_url {
                match fetch_and_display_changelog(url).await {
                    Ok(()) => {}
                    Err(e) => {
                        eprintln!("Failed to fetch changelog: {}", e);
                        println!("\nChangelog URL: {}", url);
                    }
                }
            } else {
                println!("\nNo changelog available for this version.");
            }
        }

        // If check-only mode, stop here
        if self.check {
            println!("\nRun `cortex upgrade` to install this version.");
            return Ok(());
        }

        // Confirm before proceeding
        if !self.yes && !self.force {
            print!("\nProceed with upgrade? [y/N] ");
            stdout().flush()?;
            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;
            if !input.trim().eq_ignore_ascii_case("y") {
                println!("Upgrade cancelled.");
                return Ok(());
            }
        }

        // Perform the upgrade
        perform_upgrade(&manager, &info).await
    }
}

/// Check for a specific version
async fn check_specific_version(manager: &UpdateManager, version: &str) -> Result<UpdateInfo> {
    let client = cortex_update::CortexSoftwareClient::new();
    let release = client
        .get_release(version)
        .await
        .context(format!("Version {} not found", version))?;

    let release_clone = release.clone();
    let asset = release_clone
        .asset_for_current_platform()
        .ok_or_else(|| anyhow::anyhow!("No download available for this platform"))?;

    Ok(UpdateInfo {
        current_version: CLI_VERSION.to_string(),
        latest_version: release.version,
        channel: release.channel,
        changelog_url: release.changelog_url,
        release_notes: release.release_notes,
        asset: asset.clone(),
        install_method: manager.install_method(),
    })
}

/// Perform the actual upgrade
async fn perform_upgrade(manager: &UpdateManager, info: &UpdateInfo) -> Result<()> {
    println!("\nDownloading v{}...", info.latest_version);
    println!("  Size: {} bytes", info.asset.size);

    // Download with progress
    let download = manager
        .download_update(info, |progress| {
            let pct = (progress.downloaded as f64 / progress.total as f64 * 100.0) as u32;
            print!(
                "\r  Downloading... {}% ({}/{})",
                pct, progress.downloaded, progress.total
            );
            let _ = stdout().flush();
        })
        .await
        .context("Download failed")?;

    println!("\n  Downloaded successfully");

    // Verify checksum
    print!("Verifying checksum... ");
    stdout().flush()?;
    let mut download = download;
    manager
        .verify(&mut download, &info.asset.sha256)
        .await
        .context("Checksum verification failed")?;
    println!("✓");

    // Install
    print!("Installing... ");
    stdout().flush()?;
    let outcome = manager
        .install(&download)
        .await
        .context("Installation failed")?;
    println!("✓");

    match outcome {
        UpdateOutcome::Updated { from, to } => {
            println!("\n✓ Successfully upgraded from v{} to v{}!", from, to);
            println!("  Run `cortex --version` to verify.");
        }
        UpdateOutcome::RequiresRestart => {
            println!("\n✓ Update installed. Please restart Cortex to complete.");
        }
        _ => {}
    }

    Ok(())
}

/// Simple semver comparison (returns -1, 0, or 1)
fn semver_compare(a: &str, b: &str) -> i32 {
    let parse = |v: &str| -> Vec<u32> {
        v.trim_start_matches('v')
            .split('.')
            .filter_map(|s| s.parse().ok())
            .collect()
    };

    let a_parts = parse(a);
    let b_parts = parse(b);

    for (av, bv) in a_parts.iter().zip(b_parts.iter()) {
        match av.cmp(bv) {
            std::cmp::Ordering::Less => return -1,
            std::cmp::Ordering::Greater => return 1,
            std::cmp::Ordering::Equal => continue,
        }
    }

    match a_parts.len().cmp(&b_parts.len()) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Greater => 1,
        std::cmp::Ordering::Equal => 0,
    }
}

/// Print a line, handling broken pipe gracefully (Issue #1966).
/// Returns Ok(true) if printed successfully, Ok(false) if pipe was closed.
fn print_line(line: &str) -> Result<bool> {
    use std::io::ErrorKind;
    match writeln!(stdout(), "{}", line) {
        Ok(()) => {
            // Also handle flush errors for piped output
            match stdout().flush() {
                Ok(()) => Ok(true),
                Err(e) if e.kind() == ErrorKind::BrokenPipe => Ok(false),
                Err(e) => Err(e.into()),
            }
        }
        Err(e) if e.kind() == ErrorKind::BrokenPipe => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// Fetch and display changelog content from a URL
async fn fetch_and_display_changelog(url: &str) -> Result<()> {
    // Create HTTP client
    let client = create_default_client().context("Failed to create HTTP client")?;

    // Fetch the changelog content
    let response = client
        .get(url)
        .send()
        .await
        .context("Failed to send HTTP request")?;

    if !response.status().is_success() {
        return Err(anyhow::anyhow!(
            "HTTP request failed with status: {}",
            response.status()
        ));
    }

    let content = response
        .text()
        .await
        .context("Failed to read response body")?;

    // Display the changelog with broken pipe handling (Issue #1966)
    if !print_line(&format!("\n{}", "=".repeat(80)))? {
        return Ok(()); // Pipe closed, exit gracefully
    }
    if !print_line("CHANGELOG")? {
        return Ok(());
    }
    if !print_line(&"=".repeat(80))? {
        return Ok(());
    }
    if !print_line("")? {
        return Ok(());
    }

    // If it's a GitHub URL, we might need to fetch the raw content
    let display_content = if url.contains("github.com") && !url.contains("/raw/") {
        // Try to extract useful content from HTML (basic approach)
        // For GitHub releases, the content might be in HTML format
        strip_html_tags(&content)
    } else {
        content
    };

    // Display with some basic formatting
    for line in display_content.lines() {
        // Indent bullet points slightly
        let output = if line.trim_start().starts_with('-') || line.trim_start().starts_with('*') {
            format!("  {}", line.trim())
        } else if line.trim_start().starts_with('#') {
            // Headers get some spacing
            if !line.trim().is_empty() {
                format!("\n{}\n", line.trim())
            } else {
                continue;
            }
        } else {
            line.to_string()
        };

        if !print_line(&output)? {
            return Ok(()); // Pipe closed, exit gracefully
        }
    }

    let _ = print_line("");
    let _ = print_line(&"=".repeat(80));

    Ok(())
}

/// Strip HTML tags from content (basic implementation)
fn strip_html_tags(html: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    let mut in_script_or_style = false;
    let mut tag_buffer = String::new();

    for ch in html.chars() {
        match ch {
            '<' => {
                in_tag = true;
                tag_buffer.clear();
            }
            '>' => {
                in_tag = false;
                // Check if we're entering or leaving script/style tags
                let tag_lower = tag_buffer.to_lowercase();
                if tag_lower.starts_with("script") || tag_lower.starts_with("style") {
                    in_script_or_style = true;
                } else if tag_lower.starts_with("/script") || tag_lower.starts_with("/style") {
                    in_script_or_style = false;
                }
            }
            _ => {
                if in_tag {
                    tag_buffer.push(ch);
                } else if !in_script_or_style {
                    result.push(ch);
                }
            }
        }
    }

    // Clean up excessive whitespace
    result
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_semver_compare() {
        assert_eq!(semver_compare("1.0.0", "1.0.0"), 0);
        assert_eq!(semver_compare("1.0.0", "1.0.1"), -1);
        assert_eq!(semver_compare("1.0.1", "1.0.0"), 1);
        assert_eq!(semver_compare("1.0.0", "2.0.0"), -1);
        assert_eq!(semver_compare("v1.0.0", "1.0.0"), 0);
    }
}
