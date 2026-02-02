//! Uninstall command - safely remove Cortex CLI and associated data.
//!
//! Provides functionality to:
//! - Detect installation method (cargo, manual, installer)
//! - Remove binary from system paths
//! - Clean up configuration and data directories
//! - Remove shell completions
//! - Optionally keep config/data
//! - Show what would be deleted (dry-run mode)
//! - Create backup before removal

use crate::styled_output::{print_info, print_warning};
use anyhow::{Context, Result, bail};
use clap::Parser;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Uninstall CLI command.
#[derive(Debug, Parser)]
pub struct UninstallCli {
    /// Keep configuration files (config.toml, etc.)
    #[arg(long, short = 'c', conflicts_with = "purge")]
    pub keep_config: bool,

    /// Keep session data and history
    #[arg(long, short = 'd', conflicts_with = "purge")]
    pub keep_data: bool,

    /// Show what would be deleted without actually deleting
    #[arg(long)]
    pub dry_run: bool,

    /// Skip confirmation prompt
    #[arg(long, short = 'f')]
    pub force: bool,

    /// Auto-confirm (alias for --force)
    #[arg(long, short = 'y')]
    pub yes: bool,

    /// Create backup before uninstalling
    #[arg(long)]
    pub backup: bool,

    /// Complete removal: delete everything including config and data.
    /// This is equivalent to not using --keep-config or --keep-data.
    #[arg(long, short = 'p')]
    pub purge: bool,
}

/// Installation detection result.
#[derive(Debug, Clone, PartialEq)]
enum InstallMethod {
    Cargo,
    Manual,
    Installer,
    Unknown,
}

impl std::fmt::Display for InstallMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallMethod::Cargo => write!(f, "cargo install"),
            InstallMethod::Manual => write!(f, "manual"),
            InstallMethod::Installer => write!(f, "system installer"),
            InstallMethod::Unknown => write!(f, "unknown"),
        }
    }
}

/// Item to be removed during uninstall.
#[derive(Debug, Clone)]
struct RemovalItem {
    path: PathBuf,
    description: String,
    size: u64,
    requires_sudo: bool,
    category: RemovalCategory,
}

/// Category of items to remove.
#[derive(Debug, Clone, PartialEq)]
enum RemovalCategory {
    Binary,
    Config,
    Data,
    Completions,
    Plugins,
}

impl UninstallCli {
    /// Run the uninstall command.
    pub async fn run(self) -> Result<()> {
        println!("Cortex CLI Uninstaller");
        println!("{}", "=".repeat(50));

        // Detect installation method
        let install_method = detect_installation_method();
        println!("Detected installation: {install_method}");

        // Collect all items to remove
        let all_items = collect_removal_items()?;

        // Filter items based on flags
        // --purge removes everything (equivalent to neither --keep-config nor --keep-data)
        let items_to_remove: Vec<RemovalItem> = all_items
            .into_iter()
            .filter(|item| match item.category {
                RemovalCategory::Config => self.purge || !self.keep_config,
                RemovalCategory::Data => self.purge || !self.keep_data,
                _ => true,
            })
            .filter(|item| item.path.exists())
            .collect();

        if items_to_remove.is_empty() {
            println!("\nNothing to uninstall. Cortex CLI may not be installed or already removed.");
            return Ok(());
        }

        // Display what will be removed
        println!("\nItems to remove:");
        println!("{}", "-".repeat(50));

        let mut total_size: u64 = 0;
        let mut has_sudo_items = false;
        let mut items_by_category: HashMap<String, Vec<&RemovalItem>> = HashMap::new();

        for item in &items_to_remove {
            let category_name = match item.category {
                RemovalCategory::Binary => "Binaries",
                RemovalCategory::Config => "Configuration",
                RemovalCategory::Data => "Session Data",
                RemovalCategory::Completions => "Shell Completions",
                RemovalCategory::Plugins => "Plugins & Skills",
            };
            items_by_category
                .entry(category_name.to_string())
                .or_default()
                .push(item);
            total_size += item.size;
            if item.requires_sudo {
                has_sudo_items = true;
            }
        }

        for (category, items) in &items_by_category {
            println!("\n  {category}:");
            for item in items {
                let size_str = format_size(item.size);
                let sudo_indicator = if item.requires_sudo {
                    " (requires sudo)"
                } else {
                    ""
                };
                println!(
                    "    {} ({}){}",
                    item.path.display(),
                    size_str,
                    sudo_indicator
                );
                if !item.description.is_empty() {
                    println!("      └─ {}", item.description);
                }
            }
        }

        println!("\n{}", "-".repeat(50));
        println!("Total space to free: {}", format_size(total_size));

        if has_sudo_items {
            println!();
            print_warning("Some items require elevated privileges to remove.");
            println!("   You may be prompted for your password.");
        }

        // Dry run mode - stop here
        if self.dry_run {
            println!("\n[DRY RUN] No files were deleted.");
            return Ok(());
        }

        // Backup if requested
        if self.backup {
            print_info("Creating backup...");
            if let Err(e) = create_backup(&items_to_remove) {
                print_warning(&format!("Failed to create backup: {e}"));
                if !self.force && !self.yes {
                    println!("Continue without backup? [y/N]");
                    if !prompt_yes_no()? {
                        print_info("Uninstall cancelled.");
                        return Ok(());
                    }
                }
            }
        }

        // Confirm before proceeding
        if !self.force && !self.yes {
            println!("\nAre you sure you want to uninstall Cortex CLI? [y/N]");
            if !prompt_yes_no()? {
                println!("Uninstall cancelled.");
                return Ok(());
            }
        }

        // Perform the uninstall
        println!("\nUninstalling...");
        let mut errors: Vec<(PathBuf, String)> = Vec::new();
        let mut removed_count = 0;

        for item in &items_to_remove {
            print!("  Removing {}... ", item.path.display());

            match remove_item(item) {
                Ok(()) => {
                    println!("✓");
                    removed_count += 1;
                }
                Err(e) => {
                    println!("✗");
                    errors.push((item.path.clone(), e.to_string()));
                }
            }
        }

        // Clean up shell completions from rc files
        if items_to_remove
            .iter()
            .any(|i| i.category == RemovalCategory::Completions)
        {
            print_info("Cleaning shell configuration...");
            if let Err(e) = clean_shell_completions() {
                print_warning(&format!("Failed to clean shell config: {e}"));
            }
        }

        // Summary
        println!("\n{}", "=".repeat(50));
        println!("Uninstall Summary:");
        println!("  Items removed: {removed_count}/{}", items_to_remove.len());
        println!("  Space freed: {}", format_size(total_size));

        if !errors.is_empty() {
            println!();
            print_warning("Some items could not be removed:");
            for (path, error) in &errors {
                println!("    {} - {error}", path.display());
            }
            println!("\n  You may need to remove these manually or with elevated privileges.");
        } else {
            println!("\n✓ Cortex CLI has been successfully uninstalled.");
        }

        if self.keep_config {
            println!("\n  Configuration files were preserved.");
        }
        if self.keep_data {
            println!("  Session data was preserved.");
        }

        Ok(())
    }
}

/// Detect how Cortex was installed.
fn detect_installation_method() -> InstallMethod {
    let current_exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(_) => return InstallMethod::Unknown,
    };

    let exe_path = current_exe.to_string_lossy();

    // Check if in cargo bin directory
    if exe_path.contains(".cargo") && exe_path.contains("bin") {
        return InstallMethod::Cargo;
    }

    // Check common installation paths
    #[cfg(target_os = "linux")]
    {
        if exe_path.contains("/usr/local/bin") || exe_path.contains("/usr/bin") {
            return InstallMethod::Installer;
        }
        if exe_path.contains(".local/bin") {
            return InstallMethod::Manual;
        }
    }

    #[cfg(target_os = "macos")]
    {
        if exe_path.contains("/usr/local/bin") || exe_path.contains("/opt/homebrew") {
            return InstallMethod::Installer;
        }
        if exe_path.contains(".local/bin") {
            return InstallMethod::Manual;
        }
    }

    #[cfg(target_os = "windows")]
    {
        if exe_path.contains("Program Files") || exe_path.contains("AppData\\Local\\Programs") {
            return InstallMethod::Installer;
        }
    }

    InstallMethod::Manual
}

/// Collect all items that should be removed during uninstall.
fn collect_removal_items() -> Result<Vec<RemovalItem>> {
    let mut items = Vec::new();

    // Get home directory
    let home_dir = dirs::home_dir().context("Could not determine home directory")?;

    // 1. Binary locations
    items.extend(collect_binary_locations(&home_dir)?);

    // 2. Cortex home directory (~/.cortex)
    items.extend(collect_cortex_home_items(&home_dir)?);

    // 3. Platform-specific locations
    #[cfg(target_os = "windows")]
    items.extend(collect_windows_items()?);

    // 4. Shell completions
    items.extend(collect_completion_items(&home_dir)?);

    Ok(items)
}

/// Collect binary installation locations.
fn collect_binary_locations(home_dir: &Path) -> Result<Vec<RemovalItem>> {
    let mut items = Vec::new();

    // Current executable
    if let Ok(current_exe) = std::env::current_exe()
        && current_exe.exists()
    {
        items.push(RemovalItem {
            path: current_exe.clone(),
            description: "Current Cortex binary".to_string(),
            size: get_file_size(&current_exe),
            requires_sudo: path_requires_sudo(&current_exe),
            category: RemovalCategory::Binary,
        });

        // Check for backup file (cortex.old)
        let backup_path = current_exe.with_extension("old");
        if backup_path.exists() {
            items.push(RemovalItem {
                path: backup_path.clone(),
                description: "Upgrade backup binary".to_string(),
                size: get_file_size(&backup_path),
                requires_sudo: path_requires_sudo(&backup_path),
                category: RemovalCategory::Binary,
            });
        }
    }

    // Common binary locations
    let binary_locations = [
        home_dir.join(".local").join("bin").join("Cortex"),
        home_dir.join(".cargo").join("bin").join("Cortex"),
        #[cfg(not(target_os = "windows"))]
        PathBuf::from("/usr/local/bin/cortex"),
        #[cfg(target_os = "windows")]
        home_dir.join(".cargo").join("bin").join("cortex.exe"),
    ];

    for path in binary_locations {
        if path.exists() && !items.iter().any(|i| i.path == path) {
            items.push(RemovalItem {
                path: path.clone(),
                description: String::new(),
                size: get_file_size(&path),
                requires_sudo: path_requires_sudo(&path),
                category: RemovalCategory::Binary,
            });
        }
    }

    Ok(items)
}

/// Collect items from the ~/.cortex directory.
fn collect_cortex_home_items(home_dir: &Path) -> Result<Vec<RemovalItem>> {
    let mut items = Vec::new();
    let cortex_home = home_dir.join(".cortex");

    if !cortex_home.exists() {
        return Ok(items);
    }

    // Configuration files
    let config_files = [
        ("config.toml", "Main configuration file"),
        ("credentials.json", "Authentication credentials"),
        ("auth.json", "OAuth tokens"),
    ];

    for (file, desc) in config_files {
        let path = cortex_home.join(file);
        if path.exists() {
            items.push(RemovalItem {
                path: path.clone(),
                description: desc.to_string(),
                size: get_file_size(&path),
                requires_sudo: false,
                category: RemovalCategory::Config,
            });
        }
    }

    // Session data directory
    let sessions_dir = cortex_home.join("sessions");
    if sessions_dir.exists() {
        items.push(RemovalItem {
            path: sessions_dir.clone(),
            description: "Session history and data".to_string(),
            size: get_dir_size(&sessions_dir),
            requires_sudo: false,
            category: RemovalCategory::Data,
        });
    }

    // Logs directory
    let logs_dir = cortex_home.join("logs");
    if logs_dir.exists() {
        items.push(RemovalItem {
            path: logs_dir.clone(),
            description: "Log files".to_string(),
            size: get_dir_size(&logs_dir),
            requires_sudo: false,
            category: RemovalCategory::Data,
        });
    }

    // Plugins directory
    let plugins_dir = cortex_home.join("plugins");
    if plugins_dir.exists() {
        items.push(RemovalItem {
            path: plugins_dir.clone(),
            description: "Installed plugins".to_string(),
            size: get_dir_size(&plugins_dir),
            requires_sudo: false,
            category: RemovalCategory::Plugins,
        });
    }

    // Skills directory
    let skills_dir = cortex_home.join("skills");
    if skills_dir.exists() {
        items.push(RemovalItem {
            path: skills_dir.clone(),
            description: "Custom skills".to_string(),
            size: get_dir_size(&skills_dir),
            requires_sudo: false,
            category: RemovalCategory::Plugins,
        });
    }

    // MCP servers directory
    let mcp_dir = cortex_home.join("mcp");
    if mcp_dir.exists() {
        items.push(RemovalItem {
            path: mcp_dir.clone(),
            description: "MCP server configurations".to_string(),
            size: get_dir_size(&mcp_dir),
            requires_sudo: false,
            category: RemovalCategory::Config,
        });
    }

    // Agents directory
    let agents_dir = cortex_home.join("agents");
    if agents_dir.exists() {
        items.push(RemovalItem {
            path: agents_dir.clone(),
            description: "Custom agents".to_string(),
            size: get_dir_size(&agents_dir),
            requires_sudo: false,
            category: RemovalCategory::Plugins,
        });
    }

    // Cache directory
    let cache_dir = cortex_home.join("cache");
    if cache_dir.exists() {
        items.push(RemovalItem {
            path: cache_dir.clone(),
            description: "Cached data".to_string(),
            size: get_dir_size(&cache_dir),
            requires_sudo: false,
            category: RemovalCategory::Data,
        });
    }

    // If nothing specific was found but the directory exists, add it entirely
    // (this catches any subdirectories we might have missed)
    if items
        .iter()
        .filter(|i| i.path.starts_with(&cortex_home))
        .count()
        == 0
    {
        items.push(RemovalItem {
            path: cortex_home.clone(),
            description: "Cortex home directory".to_string(),
            size: get_dir_size(&cortex_home),
            requires_sudo: false,
            category: RemovalCategory::Config,
        });
    } else {
        // Add the parent directory itself at the end (to be removed after contents)
        items.push(RemovalItem {
            path: cortex_home,
            description: "Cortex home directory (if empty)".to_string(),
            size: 0,
            requires_sudo: false,
            category: RemovalCategory::Config,
        });
    }

    Ok(items)
}

/// Collect Windows-specific locations.
#[cfg(target_os = "windows")]
fn collect_windows_items() -> Result<Vec<RemovalItem>> {
    let mut items = Vec::new();

    // %LOCALAPPDATA%\cortex
    if let Some(local_app_data) = dirs::data_local_dir() {
        let cortex_dir = local_app_data.join("Cortex");
        if cortex_dir.exists() {
            items.push(RemovalItem {
                path: cortex_dir.clone(),
                description: "Windows local app data".to_string(),
                size: get_dir_size(&cortex_dir),
                requires_sudo: false,
                category: RemovalCategory::Data,
            });
        }
    }

    // %APPDATA%\cortex
    if let Some(roaming_data) = dirs::config_dir() {
        let cortex_dir = roaming_data.join("Cortex");
        if cortex_dir.exists() {
            items.push(RemovalItem {
                path: cortex_dir.clone(),
                description: "Windows roaming app data".to_string(),
                size: get_dir_size(&cortex_dir),
                requires_sudo: false,
                category: RemovalCategory::Config,
            });
        }
    }

    Ok(items)
}

/// Collect shell completion file locations.
fn collect_completion_items(home_dir: &Path) -> Result<Vec<RemovalItem>> {
    let mut items = Vec::new();

    #[cfg(not(target_os = "windows"))]
    {
        // Bash completions
        let bash_completions = [
            home_dir.join(".local/share/bash-completion/completions/cortex"),
            PathBuf::from("/etc/bash_completion.d/cortex"),
            PathBuf::from("/usr/local/etc/bash_completion.d/cortex"),
        ];

        for path in bash_completions {
            if path.exists() {
                items.push(RemovalItem {
                    path: path.clone(),
                    description: "Bash completion script".to_string(),
                    size: get_file_size(&path),
                    requires_sudo: path_requires_sudo(&path),
                    category: RemovalCategory::Completions,
                });
            }
        }

        // Zsh completions
        let zsh_completions = [
            home_dir.join(".zfunc/_cortex"),
            home_dir.join(".local/share/zsh/site-functions/_cortex"),
            PathBuf::from("/usr/local/share/zsh/site-functions/_cortex"),
        ];

        for path in zsh_completions {
            if path.exists() {
                items.push(RemovalItem {
                    path: path.clone(),
                    description: "Zsh completion script".to_string(),
                    size: get_file_size(&path),
                    requires_sudo: path_requires_sudo(&path),
                    category: RemovalCategory::Completions,
                });
            }
        }

        // Fish completions
        let fish_completion = home_dir.join(".config/fish/completions/cortex.fish");
        if fish_completion.exists() {
            items.push(RemovalItem {
                path: fish_completion.clone(),
                description: "Fish completion script".to_string(),
                size: get_file_size(&fish_completion),
                requires_sudo: false,
                category: RemovalCategory::Completions,
            });
        }
    }

    #[cfg(target_os = "windows")]
    {
        // PowerShell profile additions are handled separately
        // Check for PowerShell completion module
        if let Some(documents) = dirs::document_dir() {
            let ps_module = documents
                .join("PowerShell")
                .join("Modules")
                .join("CortexCompletion");
            if ps_module.exists() {
                items.push(RemovalItem {
                    path: ps_module.clone(),
                    description: "PowerShell completion module".to_string(),
                    size: get_dir_size(&ps_module),
                    requires_sudo: false,
                    category: RemovalCategory::Completions,
                });
            }
        }
    }

    Ok(items)
}

/// Get the size of a file.
fn get_file_size(path: &Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Get the total size of a directory recursively.
fn get_dir_size(path: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                total += get_dir_size(&entry_path);
            } else {
                total += get_file_size(&entry_path);
            }
        }
    }
    total
}

/// Format bytes as human-readable size.
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} bytes")
    }
}

/// Check if a path requires sudo to modify.
fn path_requires_sudo(path: &Path) -> bool {
    let path_str = path.to_string_lossy();

    #[cfg(not(target_os = "windows"))]
    {
        path_str.starts_with("/usr/local/")
            || path_str.starts_with("/usr/bin/")
            || path_str.starts_with("/etc/")
            || path_str.starts_with("/opt/")
    }

    #[cfg(target_os = "windows")]
    {
        path_str.contains("Program Files") || path_str.contains("Windows")
    }
}

/// Prompt user for yes/no confirmation.
fn prompt_yes_no() -> Result<bool> {
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    Ok(input.trim().eq_ignore_ascii_case("y") || input.trim().eq_ignore_ascii_case("yes"))
}

/// Create a backup of items before removal.
fn create_backup(items: &[RemovalItem]) -> Result<()> {
    let backup_dir = dirs::home_dir()
        .context("Could not determine home directory")?
        .join(".cortex-backup")
        .join(chrono::Local::now().format("%Y%m%d_%H%M%S").to_string());

    fs::create_dir_all(&backup_dir)?;

    println!("  Backup location: {}", backup_dir.display());

    for item in items {
        if !item.path.exists() {
            continue;
        }

        let relative_path = item
            .path
            .strip_prefix(dirs::home_dir().unwrap_or_default())
            .unwrap_or(&item.path);
        let backup_path = backup_dir.join(relative_path);

        if let Some(parent) = backup_path.parent() {
            fs::create_dir_all(parent)?;
        }

        if item.path.is_dir() {
            copy_dir_all(&item.path, &backup_path)?;
        } else {
            fs::copy(&item.path, &backup_path)?;
        }
    }

    println!("  ✓ Backup created successfully");
    Ok(())
}

/// Copy a directory recursively.
fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_all(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// Remove a single item (file or directory).
fn remove_item(item: &RemovalItem) -> Result<()> {
    // Safety check: never delete root or home directory
    validate_path_safety(&item.path)?;

    if !item.path.exists() {
        return Ok(());
    }

    if item.path.is_dir() {
        // For the cortex home directory, only remove if empty
        if item.description.contains("if empty") {
            if fs::read_dir(&item.path)?.next().is_none() {
                fs::remove_dir(&item.path)?;
            }
        } else {
            fs::remove_dir_all(&item.path)?;
        }
    } else {
        fs::remove_file(&item.path)?;
    }

    Ok(())
}

/// Validate that a path is safe to delete.
fn validate_path_safety(path: &Path) -> Result<()> {
    let path_str = path.to_string_lossy();
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let canonical_str = canonical.to_string_lossy();

    // Never delete root directories
    let forbidden_paths = [
        "/",
        "/home",
        "/Users",
        "/root",
        "C:\\",
        "C:\\Windows",
        "C:\\Users",
        "C:\\Program Files",
    ];

    for forbidden in forbidden_paths {
        if canonical_str == forbidden || path_str == forbidden {
            bail!("Refusing to delete protected path: {}", path.display());
        }
    }

    // Never delete home directory itself
    if let Some(home) = dirs::home_dir()
        && canonical == home
    {
        bail!("Refusing to delete home directory");
    }

    // Ensure path contains "Cortex" somewhere (sanity check)
    if !path_str.to_lowercase().contains("Cortex") {
        // Allow common binary locations even without "Cortex" in parent path
        let is_binary = path
            .file_name()
            .map(|n| {
                let name = n.to_string_lossy().to_lowercase();
                name == "Cortex" || name == "cortex.exe" || name == "cortex.old"
            })
            .unwrap_or(false);

        if !is_binary {
            bail!(
                "Path does not appear to be Cortex-related: {}",
                path.display()
            );
        }
    }

    Ok(())
}

/// Clean up shell completion references from rc files.
fn clean_shell_completions() -> Result<()> {
    let home_dir = dirs::home_dir().context("Could not determine home directory")?;

    #[cfg(not(target_os = "windows"))]
    {
        // Clean .bashrc
        let bashrc = home_dir.join(".bashrc");
        if bashrc.exists() {
            clean_rc_file(
                &bashrc,
                &["Cortex", "# cortex", "eval \"$(cortex completion"],
            )?;
        }

        // Clean .zshrc
        let zshrc = home_dir.join(".zshrc");
        if zshrc.exists() {
            clean_rc_file(
                &zshrc,
                &["Cortex", "# cortex", "eval \"$(cortex completion"],
            )?;
        }

        // Clean .bash_profile
        let bash_profile = home_dir.join(".bash_profile");
        if bash_profile.exists() {
            clean_rc_file(&bash_profile, &["Cortex", "# cortex"])?;
        }

        // Clean .profile
        let profile = home_dir.join(".profile");
        if profile.exists() {
            clean_rc_file(&profile, &["Cortex"])?;
        }

        // Clean fish config
        let fish_config = home_dir.join(".config/fish/config.fish");
        if fish_config.exists() {
            clean_rc_file(&fish_config, &["Cortex"])?;
        }
    }

    #[cfg(target_os = "windows")]
    {
        // Note: Cleaning PowerShell profile is more complex and should be done carefully
        println!("  Note: You may need to manually remove Cortex from your PowerShell profile.");
    }

    Ok(())
}

/// Remove lines containing cortex-related content from an rc file.
fn clean_rc_file(path: &Path, patterns: &[&str]) -> Result<()> {
    let content = fs::read_to_string(path)?;
    let mut new_lines: Vec<&str> = Vec::new();
    let mut removed = false;

    for line in content.lines() {
        let should_remove = patterns.iter().any(|p| {
            line.contains(p)
                && (line.contains("completion")
                    || line.contains("source")
                    || line.contains("eval")
                    || line.trim().starts_with('#') && line.to_lowercase().contains("Cortex"))
        });

        if should_remove {
            removed = true;
        } else {
            new_lines.push(line);
        }
    }

    if removed {
        let new_content = new_lines.join("\n");
        fs::write(path, new_content)?;
        println!("  Cleaned: {}", path.display());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(0), "0 bytes");
        assert_eq!(format_size(512), "512 bytes");
        assert_eq!(format_size(1024), "1.00 KB");
        assert_eq!(format_size(1536), "1.50 KB");
        assert_eq!(format_size(1048576), "1.00 MB");
        assert_eq!(format_size(1073741824), "1.00 GB");
    }

    #[test]
    fn test_validate_path_safety() {
        // Test that root paths are rejected
        #[cfg(unix)]
        {
            assert!(validate_path_safety(Path::new("/")).is_err());
            assert!(validate_path_safety(Path::new("/home")).is_err());
        }
        #[cfg(windows)]
        {
            assert!(validate_path_safety(Path::new("C:\\")).is_err());
            assert!(validate_path_safety(Path::new("C:\\Windows")).is_err());
        }

        // Test binary names (these don't exist so may fail canonicalization but shouldn't panic)
        #[cfg(unix)]
        {
            let binary_path = Path::new("/usr/local/bin/cortex");
            let _ = validate_path_safety(binary_path); // May succeed or fail depending on path existence
        }
        #[cfg(windows)]
        {
            let binary_path = Path::new("C:\\Program Files\\cortex\\cortex.exe");
            let _ = validate_path_safety(binary_path); // May succeed or fail depending on path existence
        }
    }

    #[test]
    fn test_detect_installation_method() {
        // This is somewhat environment-dependent
        let method = detect_installation_method();
        // Just ensure it returns something valid
        match method {
            InstallMethod::Cargo
            | InstallMethod::Manual
            | InstallMethod::Installer
            | InstallMethod::Unknown => {}
        }
    }
}
