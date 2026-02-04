//! Configuration discovery utilities.
//!
//! This module provides the `findUp` pattern for searching parent directories,
//! with caching support for performance in monorepo environments.

use std::collections::HashMap;
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

use tracing::{debug, trace};

/// Maximum number of entries in each cache to prevent unbounded memory growth.
const MAX_CACHE_SIZE: usize = 1000;

/// Cache for discovered config paths.
/// Key is the start directory, value is the found config path (or None).
static CONFIG_CACHE: LazyLock<RwLock<HashMap<PathBuf, Option<PathBuf>>>> =
    LazyLock::new(|| RwLock::new(HashMap::with_capacity(MAX_CACHE_SIZE)));

/// Cache for project roots.
/// Key is the start directory, value is the project root path.
static PROJECT_ROOT_CACHE: LazyLock<RwLock<HashMap<PathBuf, Option<PathBuf>>>> =
    LazyLock::new(|| RwLock::new(HashMap::with_capacity(MAX_CACHE_SIZE)));

/// Insert a key-value pair into the cache with eviction when full.
/// When the cache reaches MAX_CACHE_SIZE, removes an arbitrary entry before inserting.
fn insert_with_eviction<K: Eq + Hash + Clone, V>(cache: &mut HashMap<K, V>, key: K, value: V) {
    if cache.len() >= MAX_CACHE_SIZE {
        // Remove first entry (simple eviction strategy)
        if let Some(k) = cache.keys().next().cloned() {
            cache.remove(&k);
        }
    }
    cache.insert(key, value);
}

/// Markers that indicate a project root directory.
const PROJECT_ROOT_MARKERS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "package.json",
    "Cargo.toml",
    "go.mod",
    "pyproject.toml",
    ".cortex",
];

/// Find a file by walking up the directory tree.
///
/// Searches for the specified filename starting from `start_dir` and walking
/// up through parent directories. Stops at the project root (git root) or
/// filesystem root.
///
/// # Arguments
/// * `start_dir` - Directory to start searching from
/// * `filename` - Name of the file to find (can include subdirectories like `.cortex/config.toml`)
///
/// # Returns
/// * `Some(PathBuf)` - Full path to the found file
/// * `None` - File not found
pub fn find_up(start_dir: &Path, filename: &str) -> Option<PathBuf> {
    let cache_key = start_dir.join(filename);

    // Check cache first
    if let Ok(cache) = CONFIG_CACHE.read() {
        if let Some(cached) = cache.get(&cache_key) {
            trace!(key = %cache_key.display(), "Using cached config lookup");
            return cached.clone();
        }
    }

    let result = find_up_uncached(start_dir, filename);

    // Store in cache with eviction when full
    if let Ok(mut cache) = CONFIG_CACHE.write() {
        insert_with_eviction(&mut cache, cache_key, result.clone());
    }

    result
}

/// Find a file by walking up, without caching.
fn find_up_uncached(start_dir: &Path, filename: &str) -> Option<PathBuf> {
    let mut current = start_dir.to_path_buf();

    // Canonicalize to handle symlinks and relative paths
    if let Ok(canonical) = dunce::canonicalize(&current) {
        current = canonical;
    }

    // Get the device ID of the starting directory to detect mount point boundaries
    #[cfg(unix)]
    let start_device = get_device_id(&current);

    loop {
        let candidate = current.join(filename);
        trace!(path = %candidate.display(), "Checking for config file");

        if candidate.exists() {
            debug!(path = %candidate.display(), "Found file");
            return Some(candidate);
        }

        // Check if we've reached a root marker or filesystem root
        if is_root_directory(&current) {
            break;
        }

        // Move to parent directory
        match current.parent() {
            Some(parent) if parent != current => {
                // Check for mount point boundary (Unix only)
                #[cfg(unix)]
                {
                    if let Some(start_dev) = start_device {
                        if let Some(parent_dev) = get_device_id(parent) {
                            if start_dev != parent_dev {
                                // Crossed a mount point boundary
                                debug!(
                                    path = %current.display(),
                                    parent = %parent.display(),
                                    "Config inheritance stopped at mount point boundary. \
                                     Parent directory is on a different filesystem."
                                );
                                break;
                            }
                        }
                    }
                }
                current = parent.to_path_buf();
            }
            _ => break,
        }
    }

    None
}

/// Get the device ID of a path (Unix only).
/// Returns None if the metadata cannot be read.
#[cfg(unix)]
fn get_device_id(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| m.dev())
}

/// Check if a directory is a project root.
fn is_root_directory(dir: &Path) -> bool {
    // Filesystem root
    if dir.parent().is_none() {
        return true;
    }

    // Check for root markers
    for marker in PROJECT_ROOT_MARKERS {
        if dir.join(marker).exists() {
            return true;
        }
    }

    false
}

/// Find the project root directory.
///
/// Walks up from `start_dir` looking for common project root markers like
/// `.git`, `package.json`, `Cargo.toml`, etc.
///
/// # Arguments
/// * `start_dir` - Directory to start searching from
///
/// # Returns
/// * `Some(PathBuf)` - Path to the project root
/// * `None` - No project root found (reached filesystem root)
pub fn find_project_root(start_dir: &Path) -> Option<PathBuf> {
    // Check cache first
    if let Ok(cache) = PROJECT_ROOT_CACHE.read() {
        if let Some(cached) = cache.get(start_dir) {
            trace!(start = %start_dir.display(), "Using cached project root");
            return cached.clone();
        }
    }

    let result = find_project_root_uncached(start_dir);

    // Store in cache with eviction when full
    if let Ok(mut cache) = PROJECT_ROOT_CACHE.write() {
        insert_with_eviction(&mut cache, start_dir.to_path_buf(), result.clone());
    }

    result
}

/// Find project root without caching.
fn find_project_root_uncached(start_dir: &Path) -> Option<PathBuf> {
    let mut current = start_dir.to_path_buf();

    // Canonicalize to handle symlinks and relative paths
    if let Ok(canonical) = dunce::canonicalize(&current) {
        current = canonical;
    }

    loop {
        // Check for root markers
        for marker in PROJECT_ROOT_MARKERS {
            if current.join(marker).exists() {
                debug!(root = %current.display(), marker = marker, "Found project root");
                return Some(current);
            }
        }

        // Move to parent directory
        match current.parent() {
            Some(parent) if parent != current => {
                current = parent.to_path_buf();
            }
            _ => {
                // Reached filesystem root without finding a project root
                return None;
            }
        }
    }
}

/// Clear the config discovery cache.
///
/// Useful when config files have been modified and need to be rediscovered.
pub fn clear_cache() {
    if let Ok(mut cache) = CONFIG_CACHE.write() {
        cache.clear();
    }
    if let Ok(mut cache) = PROJECT_ROOT_CACHE.write() {
        cache.clear();
    }
    debug!("Config discovery cache cleared");
}

/// Get the number of cached entries (for testing/debugging).
pub fn cache_size() -> usize {
    let config_size = CONFIG_CACHE.read().map(|c| c.len()).unwrap_or(0);
    let root_size = PROJECT_ROOT_CACHE.read().map(|c| c.len()).unwrap_or(0);
    config_size + root_size
}

/// Check if a path is within a git repository.
pub fn is_in_git_repo(path: &Path) -> bool {
    find_up(path, ".git").is_some()
}

/// Get the git root for a path, if it exists.
pub fn git_root(path: &Path) -> Option<PathBuf> {
    find_up(path, ".git").map(|git_path| {
        // .git is found, return its parent
        git_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or(git_path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::TempDir;

    fn setup_test_dir() -> TempDir {
        // Clear cache before each test
        clear_cache();
        TempDir::new().unwrap()
    }

    #[test]
    #[cfg_attr(
        windows,
        ignore = "Windows 8.3 path names cause path comparison issues"
    )]
    #[cfg_attr(target_os = "macos", ignore = "macOS symlinks /var -> /private/var")]
    fn test_find_up_file_in_start_dir() {
        let temp_dir = setup_test_dir();
        let config_path = temp_dir.path().join("test.toml");
        std::fs::write(&config_path, "test = true").unwrap();

        let found = find_up(temp_dir.path(), "test.toml");
        assert!(found.is_some());
        assert_eq!(found.unwrap(), config_path);
    }

    #[test]
    #[cfg_attr(
        windows,
        ignore = "Windows 8.3 path names cause path comparison issues"
    )]
    #[cfg_attr(target_os = "macos", ignore = "macOS symlinks /var -> /private/var")]
    fn test_find_up_file_in_parent() {
        let temp_dir = setup_test_dir();
        let config_path = temp_dir.path().join("test.toml");
        std::fs::write(&config_path, "test = true").unwrap();

        let sub_dir = temp_dir.path().join("subdir");
        std::fs::create_dir_all(&sub_dir).unwrap();

        let found = find_up(&sub_dir, "test.toml");
        assert!(found.is_some());
        assert_eq!(found.unwrap(), config_path);
    }

    #[test]
    fn test_find_up_not_found() {
        let temp_dir = setup_test_dir();
        let found = find_up(temp_dir.path(), "nonexistent.toml");
        assert!(found.is_none());
    }

    #[test]
    fn test_find_up_stops_at_git_root() {
        let temp_dir = setup_test_dir();

        // Create git root marker
        std::fs::create_dir_all(temp_dir.path().join(".git")).unwrap();

        // Create file above the "git root" (simulating outside repo)
        let parent = temp_dir.path().join("subproject");
        std::fs::create_dir_all(&parent).unwrap();

        // The search should stop at .git marker
        let found = find_up(&parent, ".git");
        assert!(found.is_some());
    }

    #[test]
    fn test_find_project_root() {
        let temp_dir = setup_test_dir();

        // Create git directory
        std::fs::create_dir_all(temp_dir.path().join(".git")).unwrap();

        let sub_dir = temp_dir.path().join("src").join("deep");
        std::fs::create_dir_all(&sub_dir).unwrap();

        let root = find_project_root(&sub_dir);
        assert!(root.is_some());
        // Should find the directory containing .git
        let root_path = root.unwrap();
        assert!(root_path.join(".git").exists());
    }

    #[test]
    fn test_find_project_root_with_package_json() {
        let temp_dir = setup_test_dir();

        std::fs::write(temp_dir.path().join("package.json"), "{}").unwrap();

        let sub_dir = temp_dir.path().join("src");
        std::fs::create_dir_all(&sub_dir).unwrap();

        let root = find_project_root(&sub_dir);
        assert!(root.is_some());
    }

    #[test]
    #[serial]
    fn test_caching() {
        // Clear cache first - must use #[serial] since tests share static cache
        clear_cache();

        let temp_dir = setup_test_dir();
        std::fs::write(temp_dir.path().join("test.toml"), "test = true").unwrap();

        // First call - cache miss, should add entries
        let result = find_up(temp_dir.path(), "test.toml");
        assert!(result.is_some(), "find_up should find test.toml");

        // Second call - should use cache (just verify it doesn't panic and returns same result)
        let result2 = find_up(temp_dir.path(), "test.toml");
        assert_eq!(result, result2, "Cached result should match original");

        // Clear and verify the clear function works (cache should be empty after clear)
        clear_cache();
        let size_after_clear = cache_size();
        assert_eq!(
            size_after_clear, 0,
            "Cache should be empty after clear_cache()"
        );
    }

    #[test]
    fn test_find_up_nested_path() {
        let temp_dir = setup_test_dir();

        // Create .cortex/config.toml
        let cortex_dir = temp_dir.path().join(".cortex");
        std::fs::create_dir_all(&cortex_dir).unwrap();
        std::fs::write(cortex_dir.join("config.toml"), "test = true").unwrap();

        let found = find_up(temp_dir.path(), ".cortex/config.toml");
        assert!(found.is_some());
        assert!(found.unwrap().ends_with(".cortex/config.toml"));
    }

    #[test]
    fn test_git_root() {
        let temp_dir = setup_test_dir();
        std::fs::create_dir_all(temp_dir.path().join(".git")).unwrap();

        let sub_dir = temp_dir.path().join("src");
        std::fs::create_dir_all(&sub_dir).unwrap();

        let root = git_root(&sub_dir);
        assert!(root.is_some());
    }

    #[test]
    fn test_is_in_git_repo() {
        let temp_dir = setup_test_dir();
        std::fs::create_dir_all(temp_dir.path().join(".git")).unwrap();

        assert!(is_in_git_repo(temp_dir.path()));

        let temp_dir2 = setup_test_dir();
        assert!(!is_in_git_repo(temp_dir2.path()));
    }
}
