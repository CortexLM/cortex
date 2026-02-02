//! External directory protection.
//!
//! Detects and prompts for permission when accessing paths outside the project directory.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Checker for external directory access.
#[derive(Debug)]
pub struct ExternalDirectoryChecker {
    /// Project root directory.
    project_root: PathBuf,
    /// Additional allowed directories.
    allowed_dirs: Arc<RwLock<Vec<PathBuf>>>,
}

impl ExternalDirectoryChecker {
    /// Create a new checker with the given project root.
    pub fn new(project_root: PathBuf) -> Self {
        Self {
            project_root,
            allowed_dirs: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Check if a path is external to the project.
    pub fn is_external(&self, path: &Path) -> bool {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let root = self
            .project_root
            .canonicalize()
            .unwrap_or_else(|_| self.project_root.clone());

        !path.starts_with(&root)
    }

    /// Check if a path is allowed (either in project or explicitly allowed).
    pub async fn is_allowed(&self, path: &Path) -> bool {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

        // Check project root
        let root = self
            .project_root
            .canonicalize()
            .unwrap_or_else(|_| self.project_root.clone());
        if path.starts_with(&root) {
            return true;
        }

        // Check allowed directories
        let allowed = self.allowed_dirs.read().await;
        for dir in allowed.iter() {
            let dir = dir.canonicalize().unwrap_or_else(|_| dir.clone());
            if path.starts_with(&dir) {
                return true;
            }
        }

        false
    }

    /// Add an allowed directory.
    pub async fn allow_directory(&self, path: PathBuf) {
        let mut allowed = self.allowed_dirs.write().await;
        let path = path.canonicalize().unwrap_or(path);
        if !allowed.contains(&path) {
            allowed.push(path);
        }
    }

    /// Get the project root.
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    /// Get relative path from project root, or absolute if external.
    pub fn relative_path(&self, path: &Path) -> PathBuf {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let root = self
            .project_root
            .canonicalize()
            .unwrap_or_else(|_| self.project_root.clone());

        path.strip_prefix(&root)
            .map(|p| p.to_path_buf())
            .unwrap_or(path)
    }
}

impl Clone for ExternalDirectoryChecker {
    fn clone(&self) -> Self {
        Self {
            project_root: self.project_root.clone(),
            allowed_dirs: Arc::clone(&self.allowed_dirs),
        }
    }
}

/// Check if a path is external to a given root.
pub fn is_external_path(path: &Path, root: &Path) -> bool {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

    !path.starts_with(&root)
}

/// Get common external directories that might need access.
pub fn common_external_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // Home directory
    if let Some(home) = dirs::home_dir() {
        // Common config locations
        dirs.push(home.join(".config"));
        dirs.push(home.join(".local"));

        #[cfg(target_os = "macos")]
        {
            dirs.push(home.join("Library/Application Support"));
            dirs.push(home.join("Library/Preferences"));
        }

        #[cfg(target_os = "windows")]
        {
            if let Some(app_data) = dirs::config_dir() {
                dirs.push(app_data);
            }
        }
    }

    // Temp directory
    dirs.push(std::env::temp_dir());

    dirs
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_is_external() {
        let project = tempdir().unwrap();
        let checker = ExternalDirectoryChecker::new(project.path().to_path_buf());

        // Create a file inside the project
        let src_dir = project.path().join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        let inside = src_dir.join("main.rs");
        std::fs::write(&inside, "fn main() {}").unwrap();
        assert!(!checker.is_external(&inside));

        // Path outside project (use a file that actually exists)
        let outside = if cfg!(windows) {
            PathBuf::from("C:\\Windows\\System32\\cmd.exe")
        } else {
            PathBuf::from("/bin/sh")
        };
        if outside.exists() {
            assert!(checker.is_external(&outside));
        }
    }

    #[tokio::test]
    async fn test_allowed_directories() {
        let project = tempdir().unwrap();
        let checker = ExternalDirectoryChecker::new(project.path().to_path_buf());

        let external = tempdir().unwrap();
        let external_file = external.path().join("file.txt");
        // Create the file so canonicalize works
        std::fs::write(&external_file, "test").unwrap();

        // Not allowed initially
        assert!(!checker.is_allowed(&external_file).await);

        // Allow the directory
        checker.allow_directory(external.path().to_path_buf()).await;

        // Now allowed
        assert!(checker.is_allowed(&external_file).await);
    }

    #[test]
    fn test_relative_path() {
        let project = tempdir().unwrap();
        let checker = ExternalDirectoryChecker::new(project.path().to_path_buf());

        // Create the file so canonicalize works
        let src_dir = project.path().join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        let inside = src_dir.join("main.rs");
        std::fs::write(&inside, "fn main() {}").unwrap();

        let relative = checker.relative_path(&inside);
        let expected = PathBuf::from("src").join("main.rs");
        assert_eq!(relative, expected);

        // Use a path guaranteed to be outside the temp dir
        let outside = if cfg!(windows) {
            PathBuf::from("C:\\Windows\\System32\\cmd.exe")
        } else {
            PathBuf::from("/bin/sh")
        };
        if outside.exists() {
            let result = checker.relative_path(&outside);
            // Should return the absolute path for external files
            assert!(result.is_absolute());
        }
    }
}
