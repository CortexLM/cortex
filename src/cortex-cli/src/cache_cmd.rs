//! Cache management command for Cortex CLI.
//!
//! Provides cache management functionality:
//! - Show cache size and location
//! - Clear cache data
//! - List cached items

use anyhow::Result;
use clap::Parser;
use serde::Serialize;
use std::path::PathBuf;

/// Cache CLI command.
#[derive(Debug, Parser)]
pub struct CacheCli {
    #[command(subcommand)]
    pub subcommand: Option<CacheSubcommand>,
}

/// Cache subcommands.
#[derive(Debug, clap::Subcommand)]
pub enum CacheSubcommand {
    /// Show cache information and statistics
    #[command(visible_alias = "info")]
    Show(CacheShowArgs),

    /// Clear all or part of the cache
    Clear(CacheClearArgs),

    /// Show cache size
    Size(CacheSizeArgs),

    /// List cached items
    #[command(visible_alias = "ls")]
    List(CacheListArgs),
}

/// Arguments for cache show command.
#[derive(Debug, Parser)]
pub struct CacheShowArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

/// Arguments for cache clear command.
#[derive(Debug, Parser)]
pub struct CacheClearArgs {
    /// Clear only model cache
    #[arg(long)]
    pub models: bool,

    /// Clear only response cache
    #[arg(long)]
    pub responses: bool,

    /// Clear only update check cache
    #[arg(long)]
    pub updates: bool,

    /// Skip confirmation prompt
    #[arg(long, short = 'y')]
    pub yes: bool,

    /// Dry run - show what would be deleted without actually deleting
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments for cache size command.
#[derive(Debug, Parser)]
pub struct CacheSizeArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

/// Arguments for cache list command.
#[derive(Debug, Parser)]
pub struct CacheListArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Maximum number of items to show
    #[arg(long, short = 'n', default_value = "50")]
    pub limit: usize,
}

/// Cache statistics.
#[derive(Debug, Serialize)]
struct CacheStats {
    cache_dir: PathBuf,
    exists: bool,
    total_size_bytes: u64,
    total_size_human: String,
    item_count: usize,
    categories: Vec<CacheCategory>,
}

/// Cache category stats.
#[derive(Debug, Serialize)]
struct CacheCategory {
    name: String,
    path: PathBuf,
    size_bytes: u64,
    size_human: String,
    item_count: usize,
}

/// Get the cache directory.
fn get_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .map(|c| c.join("cortex"))
        .unwrap_or_else(|| {
            dirs::home_dir()
                .map(|h| h.join(".cache").join("cortex"))
                .unwrap_or_else(|| PathBuf::from(".cache/cortex"))
        })
}

/// Calculate directory size recursively.
fn dir_size(path: &PathBuf) -> u64 {
    let mut total = 0;
    if path.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let entry_path = entry.path();
                if entry_path.is_dir() {
                    total += dir_size(&entry_path);
                } else if let Ok(meta) = entry.metadata() {
                    total += meta.len();
                }
            }
        }
    } else if let Ok(meta) = std::fs::metadata(path) {
        total = meta.len();
    }
    total
}

/// Count items in a directory.
fn count_items(path: &PathBuf) -> usize {
    if !path.exists() {
        return 0;
    }
    std::fs::read_dir(path)
        .map(|entries| entries.count())
        .unwrap_or(0)
}

/// Format bytes as human-readable string.
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
        format!("{} B", bytes)
    }
}

impl CacheCli {
    /// Run the cache command.
    pub async fn run(self) -> Result<()> {
        match self.subcommand {
            None => run_show(CacheShowArgs { json: false }).await,
            Some(CacheSubcommand::Show(args)) => run_show(args).await,
            Some(CacheSubcommand::Clear(args)) => run_clear(args).await,
            Some(CacheSubcommand::Size(args)) => run_size(args).await,
            Some(CacheSubcommand::List(args)) => run_list(args).await,
        }
    }
}

async fn run_show(args: CacheShowArgs) -> Result<()> {
    let cache_dir = get_cache_dir();
    let exists = cache_dir.exists();

    // Define cache categories
    let category_names = [
        ("models", "models"),
        ("responses", "responses"),
        ("updates", "updates"),
        ("logs", "logs"),
        ("temp", "temp"),
    ];

    let mut categories = Vec::new();
    let mut total_size = 0u64;
    let mut total_items = 0usize;

    for (name, subdir) in category_names {
        let path = cache_dir.join(subdir);
        let size = if path.exists() { dir_size(&path) } else { 0 };
        let items = count_items(&path);

        total_size += size;
        total_items += items;

        categories.push(CacheCategory {
            name: name.to_string(),
            path,
            size_bytes: size,
            size_human: format_size(size),
            item_count: items,
        });
    }

    let stats = CacheStats {
        cache_dir: cache_dir.clone(),
        exists,
        total_size_bytes: total_size,
        total_size_human: format_size(total_size),
        item_count: total_items,
        categories,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&stats)?);
        return Ok(());
    }

    println!("Cache Information");
    println!("{}", "=".repeat(50));
    println!("  Location: {}", stats.cache_dir.display());
    println!("  Exists:   {}", if stats.exists { "yes" } else { "no" });
    println!("  Total Size: {}", stats.total_size_human);
    println!("  Total Items: {}", stats.item_count);

    if exists && !stats.categories.is_empty() {
        println!();
        println!("Categories:");
        println!("{}", "-".repeat(40));
        for cat in &stats.categories {
            if cat.size_bytes > 0 || cat.item_count > 0 {
                println!(
                    "  {:<12} {:>10}  ({} items)",
                    cat.name, cat.size_human, cat.item_count
                );
            }
        }
    }

    println!();
    println!("Use 'cortex cache clear' to clear the cache.");

    Ok(())
}

async fn run_clear(args: CacheClearArgs) -> Result<()> {
    let cache_dir = get_cache_dir();

    if !cache_dir.exists() {
        println!("Cache directory does not exist. Nothing to clear.");
        return Ok(());
    }

    // Determine what to clear
    let clear_all = !args.models && !args.responses && !args.updates;

    let mut to_clear: Vec<(&str, PathBuf)> = Vec::new();

    if clear_all || args.models {
        to_clear.push(("models", cache_dir.join("models")));
    }
    if clear_all || args.responses {
        to_clear.push(("responses", cache_dir.join("responses")));
    }
    if clear_all || args.updates {
        to_clear.push(("updates", cache_dir.join("updates")));
    }
    if clear_all {
        to_clear.push(("logs", cache_dir.join("logs")));
        to_clear.push(("temp", cache_dir.join("temp")));
    }

    // Calculate size to be cleared
    let mut total_to_clear = 0u64;
    for (_, path) in &to_clear {
        if path.exists() {
            total_to_clear += dir_size(path);
        }
    }

    if total_to_clear == 0 {
        println!("Cache is already empty. Nothing to clear.");
        return Ok(());
    }

    // Show what will be cleared
    if args.dry_run {
        println!("Dry run - would clear:");
        for (name, path) in &to_clear {
            if path.exists() {
                let size = dir_size(path);
                println!("  {} ({}) - {}", name, format_size(size), path.display());
            }
        }
        println!("\nTotal: {}", format_size(total_to_clear));
        return Ok(());
    }

    // Confirm
    if !args.yes {
        println!(
            "This will clear {} of cached data.",
            format_size(total_to_clear)
        );
        println!("Are you sure? (y/N)");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    // Clear
    let mut cleared = 0u64;
    for (name, path) in &to_clear {
        if path.exists() {
            let size = dir_size(path);
            if let Err(e) = std::fs::remove_dir_all(path) {
                eprintln!("Warning: Failed to clear {}: {}", name, e);
            } else {
                cleared += size;
                println!("Cleared {} ({})", name, format_size(size));
            }
        }
    }

    println!("\nTotal cleared: {}", format_size(cleared));

    Ok(())
}

async fn run_size(args: CacheSizeArgs) -> Result<()> {
    let cache_dir = get_cache_dir();

    let size = if cache_dir.exists() {
        dir_size(&cache_dir)
    } else {
        0
    };

    if args.json {
        let output = serde_json::json!({
            "size_bytes": size,
            "size_human": format_size(size),
            "cache_dir": cache_dir.display().to_string(),
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("{}", format_size(size));
    }

    Ok(())
}

async fn run_list(args: CacheListArgs) -> Result<()> {
    let cache_dir = get_cache_dir();

    if !cache_dir.exists() {
        if args.json {
            println!("[]");
        } else {
            println!("Cache is empty.");
        }
        return Ok(());
    }

    // Collect all cached items
    let mut items: Vec<serde_json::Value> = Vec::new();

    fn collect_items(
        dir: &PathBuf,
        prefix: &str,
        items: &mut Vec<serde_json::Value>,
        limit: usize,
    ) {
        if items.len() >= limit {
            return;
        }
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                if items.len() >= limit {
                    break;
                }
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                let full_name = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{}/{}", prefix, name)
                };

                if path.is_dir() {
                    collect_items(&path, &full_name, items, limit);
                } else if let Ok(meta) = entry.metadata() {
                    items.push(serde_json::json!({
                        "name": full_name,
                        "size": meta.len(),
                        "size_human": format_size(meta.len()),
                    }));
                }
            }
        }
    }

    collect_items(&cache_dir, "", &mut items, args.limit);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&items)?);
    } else if items.is_empty() {
        println!("Cache is empty.");
    } else {
        println!("Cached Items ({}):", items.len());
        println!("{}", "-".repeat(60));
        for item in &items {
            let name = item["name"].as_str().unwrap_or("");
            let size = item["size_human"].as_str().unwrap_or("");
            println!("  {:>10}  {}", size, name);
        }
        if items.len() >= args.limit {
            println!("\n  ... (showing first {} items)", args.limit);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    // =========================================================================
    // Tests for format_size()
    // =========================================================================

    #[test]
    fn test_format_size_bytes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1), "1 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1023), "1023 B");
    }

    #[test]
    fn test_format_size_kilobytes() {
        assert_eq!(format_size(1024), "1.00 KB");
        assert_eq!(format_size(1536), "1.50 KB");
        assert_eq!(format_size(2048), "2.00 KB");
        assert_eq!(format_size(1024 * 1023), "1023.00 KB");
    }

    #[test]
    fn test_format_size_megabytes() {
        assert_eq!(format_size(1024 * 1024), "1.00 MB");
        assert_eq!(format_size(1024 * 1024 + 512 * 1024), "1.50 MB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.00 MB");
        assert_eq!(format_size(100 * 1024 * 1024), "100.00 MB");
    }

    #[test]
    fn test_format_size_gigabytes() {
        assert_eq!(format_size(1024 * 1024 * 1024), "1.00 GB");
        assert_eq!(format_size(2 * 1024 * 1024 * 1024), "2.00 GB");
        assert_eq!(
            format_size(1024 * 1024 * 1024 + 512 * 1024 * 1024),
            "1.50 GB"
        );
    }

    #[test]
    fn test_format_size_boundary_values() {
        // Just below 1 KB
        assert_eq!(format_size(1023), "1023 B");
        // Exactly 1 KB
        assert_eq!(format_size(1024), "1.00 KB");
        // Just below 1 MB
        assert_eq!(format_size(1024 * 1024 - 1), "1024.00 KB");
        // Exactly 1 MB
        assert_eq!(format_size(1024 * 1024), "1.00 MB");
        // Just below 1 GB
        assert_eq!(format_size(1024 * 1024 * 1024 - 1), "1024.00 MB");
        // Exactly 1 GB
        assert_eq!(format_size(1024 * 1024 * 1024), "1.00 GB");
    }

    // =========================================================================
    // Tests for count_items()
    // =========================================================================

    #[test]
    fn test_count_items_empty_directory() {
        let temp = tempdir().expect("Failed to create temp directory");
        let count = count_items(&temp.path().to_path_buf());
        assert_eq!(count, 0);
    }

    #[test]
    fn test_count_items_nonexistent_directory() {
        let path = PathBuf::from("/nonexistent/path/that/does/not/exist/12345");
        let count = count_items(&path);
        assert_eq!(count, 0);
    }

    #[test]
    fn test_count_items_with_files() {
        let temp = tempdir().expect("Failed to create temp directory");

        // Create files
        fs::write(temp.path().join("file1.txt"), "content1").expect("Failed to write file1");
        fs::write(temp.path().join("file2.txt"), "content2").expect("Failed to write file2");
        fs::write(temp.path().join("file3.txt"), "content3").expect("Failed to write file3");

        let count = count_items(&temp.path().to_path_buf());
        assert_eq!(count, 3);
    }

    #[test]
    fn test_count_items_with_subdirectories() {
        let temp = tempdir().expect("Failed to create temp directory");

        // Create a file and a subdirectory
        fs::write(temp.path().join("file.txt"), "content").expect("Failed to write file");
        fs::create_dir(temp.path().join("subdir")).expect("Failed to create subdir");

        // count_items only counts immediate children, not recursively
        let count = count_items(&temp.path().to_path_buf());
        assert_eq!(count, 2);
    }

    #[test]
    fn test_count_items_only_subdirectories() {
        let temp = tempdir().expect("Failed to create temp directory");

        // Create subdirectories only
        fs::create_dir(temp.path().join("subdir1")).expect("Failed to create subdir1");
        fs::create_dir(temp.path().join("subdir2")).expect("Failed to create subdir2");

        let count = count_items(&temp.path().to_path_buf());
        assert_eq!(count, 2);
    }

    // =========================================================================
    // Tests for dir_size()
    // =========================================================================

    #[test]
    fn test_dir_size_empty_directory() {
        let temp = tempdir().expect("Failed to create temp directory");
        let size = dir_size(&temp.path().to_path_buf());
        assert_eq!(size, 0);
    }

    #[test]
    fn test_dir_size_with_single_file() {
        let temp = tempdir().expect("Failed to create temp directory");

        let content = "hello world";
        fs::write(temp.path().join("test.txt"), content).expect("Failed to write test file");

        let size = dir_size(&temp.path().to_path_buf());
        assert_eq!(size, content.len() as u64);
    }

    #[test]
    fn test_dir_size_with_multiple_files() {
        let temp = tempdir().expect("Failed to create temp directory");

        let content1 = "hello";
        let content2 = "world";
        let content3 = "!";

        fs::write(temp.path().join("file1.txt"), content1).expect("Failed to write file1");
        fs::write(temp.path().join("file2.txt"), content2).expect("Failed to write file2");
        fs::write(temp.path().join("file3.txt"), content3).expect("Failed to write file3");

        let size = dir_size(&temp.path().to_path_buf());
        let expected_size = (content1.len() + content2.len() + content3.len()) as u64;
        assert_eq!(size, expected_size);
    }

    #[test]
    fn test_dir_size_recursive() {
        let temp = tempdir().expect("Failed to create temp directory");

        // Create nested directory structure
        let subdir = temp.path().join("subdir");
        fs::create_dir(&subdir).expect("Failed to create subdir");

        let nested_subdir = subdir.join("nested");
        fs::create_dir(&nested_subdir).expect("Failed to create nested subdir");

        // Create files at different levels
        let content_root = "root file content";
        let content_sub = "subdir file content";
        let content_nested = "nested file content";

        fs::write(temp.path().join("root.txt"), content_root).expect("Failed to write root file");
        fs::write(subdir.join("sub.txt"), content_sub).expect("Failed to write sub file");
        fs::write(nested_subdir.join("nested.txt"), content_nested)
            .expect("Failed to write nested file");

        let size = dir_size(&temp.path().to_path_buf());
        let expected_size = (content_root.len() + content_sub.len() + content_nested.len()) as u64;
        assert_eq!(size, expected_size);
    }

    #[test]
    fn test_dir_size_nonexistent_directory() {
        let path = PathBuf::from("/nonexistent/path/that/does/not/exist/12345");
        let size = dir_size(&path);
        assert_eq!(size, 0);
    }

    #[test]
    fn test_dir_size_single_file_not_directory() {
        let temp = tempdir().expect("Failed to create temp directory");

        let content = "file content for direct file test";
        let file_path = temp.path().join("single_file.txt");
        fs::write(&file_path, content).expect("Failed to write file");

        // dir_size should handle a file path (not directory) by returning its size
        let size = dir_size(&file_path);
        assert_eq!(size, content.len() as u64);
    }

    #[test]
    fn test_dir_size_empty_subdirectories() {
        let temp = tempdir().expect("Failed to create temp directory");

        // Create empty subdirectories
        fs::create_dir(temp.path().join("empty1")).expect("Failed to create empty1");
        fs::create_dir(temp.path().join("empty2")).expect("Failed to create empty2");

        let size = dir_size(&temp.path().to_path_buf());
        // Empty directories should contribute 0 to size
        assert_eq!(size, 0);
    }
}
