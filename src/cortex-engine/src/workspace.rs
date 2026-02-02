//! Workspace management.
//!
//! Manages workspace state including open files, project structure,
//! and file tracking.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::error::Result;

/// Workspace configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    /// Workspace root.
    pub root: PathBuf,
    /// Ignored patterns.
    pub ignore_patterns: Vec<String>,
    /// Maximum file size to track (bytes).
    pub max_file_size: u64,
    /// Maximum files to track.
    pub max_files: usize,
    /// Auto-reload on external changes.
    pub auto_reload: bool,
    /// Track git status.
    pub track_git: bool,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            ignore_patterns: vec![
                ".git/**".to_string(),
                "node_modules/**".to_string(),
                "target/**".to_string(),
                "__pycache__/**".to_string(),
                "*.pyc".to_string(),
                ".venv/**".to_string(),
                "venv/**".to_string(),
                "dist/**".to_string(),
                "build/**".to_string(),
            ],
            max_file_size: 10 * 1024 * 1024, // 10MB
            max_files: 10000,
            auto_reload: true,
            track_git: true,
        }
    }
}

/// File state in workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileState {
    /// File path.
    pub path: PathBuf,
    /// File status.
    pub status: FileStatus,
    /// Last modified time.
    pub modified_at: u64,
    /// File size in bytes.
    pub size: u64,
    /// Content hash.
    pub hash: Option<String>,
    /// Language/type.
    pub language: Option<String>,
    /// Is binary.
    pub is_binary: bool,
    /// Git status.
    pub git_status: Option<GitFileStatus>,
    /// Is open in editor.
    pub is_open: bool,
    /// Has unsaved changes.
    pub is_dirty: bool,
    /// Last accessed time.
    pub accessed_at: u64,
}

impl FileState {
    /// Create a new file state.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let now = timestamp_now();

        Self {
            path,
            status: FileStatus::Unknown,
            modified_at: now,
            size: 0,
            hash: None,
            language: None,
            is_binary: false,
            git_status: None,
            is_open: false,
            is_dirty: false,
            accessed_at: now,
        }
    }

    /// Refresh from disk.
    pub fn refresh(&mut self) -> Result<()> {
        if !self.path.exists() {
            self.status = FileStatus::Deleted;
            return Ok(());
        }

        let metadata = std::fs::metadata(&self.path)?;

        if metadata.is_dir() {
            self.status = FileStatus::Directory;
            return Ok(());
        }

        self.size = metadata.len();
        self.modified_at = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);

        self.status = FileStatus::Normal;
        self.language = detect_language(&self.path);
        self.is_binary = is_binary_file(&self.path);

        Ok(())
    }

    /// Mark as open.
    pub fn open(&mut self) {
        self.is_open = true;
        self.accessed_at = timestamp_now();
    }

    /// Mark as closed.
    pub fn close(&mut self) {
        self.is_open = false;
    }

    /// Mark as dirty (has unsaved changes).
    pub fn mark_dirty(&mut self) {
        self.is_dirty = true;
    }

    /// Mark as clean (saved).
    pub fn mark_clean(&mut self) {
        self.is_dirty = false;
    }
}

/// File status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum FileStatus {
    /// Normal file.
    Normal,
    /// Directory.
    Directory,
    /// File was created.
    Created,
    /// File was modified.
    Modified,
    /// File was deleted.
    Deleted,
    /// File was renamed.
    Renamed,
    /// Unknown status.
    #[default]
    Unknown,
}

/// Git file status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitFileStatus {
    /// Untracked.
    Untracked,
    /// Staged.
    Staged,
    /// Modified.
    Modified,
    /// Deleted.
    Deleted,
    /// Renamed.
    Renamed,
    /// Conflicted.
    Conflicted,
    /// Ignored.
    Ignored,
}

/// Workspace.
pub struct Workspace {
    /// Configuration.
    config: WorkspaceConfig,
    /// Tracked files.
    files: RwLock<HashMap<PathBuf, FileState>>,
    /// Open files.
    open_files: RwLock<HashSet<PathBuf>>,
    /// Recently accessed files.
    recent_files: RwLock<Vec<PathBuf>>,
    /// File change listeners.
    change_listeners: RwLock<Vec<Arc<dyn FileChangeListener>>>,
}

impl Workspace {
    /// Create a new workspace.
    pub fn new(config: WorkspaceConfig) -> Self {
        Self {
            config,
            files: RwLock::new(HashMap::new()),
            open_files: RwLock::new(HashSet::new()),
            recent_files: RwLock::new(Vec::new()),
            change_listeners: RwLock::new(Vec::new()),
        }
    }

    /// Create with default config.
    pub fn default_workspace() -> Self {
        Self::new(WorkspaceConfig::default())
    }

    /// Get workspace root.
    pub fn root(&self) -> &Path {
        &self.config.root
    }

    /// Scan workspace.
    pub async fn scan(&self) -> Result<ScanResult> {
        let mut files_found = 0;
        let mut dirs_found = 0;
        let mut total_size = 0u64;

        let mut files = self.files.write().await;

        for entry in walkdir::WalkDir::new(&self.config.root)
            .into_iter()
            .filter_map(std::result::Result::ok)
        {
            let path = entry.path().to_path_buf();

            // Check ignore patterns
            if self.is_ignored(&path) {
                continue;
            }

            // Check limits
            if files.len() >= self.config.max_files {
                break;
            }

            if entry.file_type().is_dir() {
                dirs_found += 1;
                continue;
            }

            // Check file size
            if let Ok(metadata) = entry.metadata() {
                if metadata.len() > self.config.max_file_size {
                    continue;
                }
                total_size += metadata.len();
            }

            let mut state = FileState::new(&path);
            let _ = state.refresh();
            files.insert(path, state);
            files_found += 1;
        }

        Ok(ScanResult {
            files_found,
            dirs_found,
            total_size,
        })
    }

    /// Check if path is ignored.
    fn is_ignored(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();

        for pattern in &self.config.ignore_patterns {
            if let Ok(glob) = glob::Pattern::new(pattern)
                && glob.matches(&path_str)
            {
                return true;
            }
        }

        false
    }

    /// Get file state.
    pub async fn get_file(&self, path: &Path) -> Option<FileState> {
        self.files.read().await.get(path).cloned()
    }

    /// Track a file.
    pub async fn track_file(&self, path: impl AsRef<Path>) -> Result<FileState> {
        let path = path.as_ref().to_path_buf();
        let mut state = FileState::new(&path);
        state.refresh()?;

        self.files.write().await.insert(path.clone(), state.clone());
        Ok(state)
    }

    /// Untrack a file.
    pub async fn untrack_file(&self, path: &Path) {
        self.files.write().await.remove(path);
        self.open_files.write().await.remove(path);
    }

    /// Open a file.
    pub async fn open_file(&self, path: impl AsRef<Path>) -> Result<FileState> {
        let path = path.as_ref().to_path_buf();

        let mut files = self.files.write().await;
        let state = files
            .entry(path.clone())
            .or_insert_with(|| FileState::new(&path));
        state.refresh()?;
        state.open();

        self.open_files.write().await.insert(path.clone());
        self.add_recent(&path).await;

        Ok(state.clone())
    }

    /// Close a file.
    pub async fn close_file(&self, path: &Path) {
        if let Some(state) = self.files.write().await.get_mut(path) {
            state.close();
        }
        self.open_files.write().await.remove(path);
    }

    /// Get open files.
    pub async fn open_files(&self) -> Vec<PathBuf> {
        self.open_files.read().await.iter().cloned().collect()
    }

    /// Add to recent files.
    async fn add_recent(&self, path: &Path) {
        let mut recent = self.recent_files.write().await;
        recent.retain(|p| p != path);
        recent.insert(0, path.to_path_buf());
        recent.truncate(50); // Keep last 50
    }

    /// Get recent files.
    pub async fn recent_files(&self, limit: usize) -> Vec<PathBuf> {
        self.recent_files
            .read()
            .await
            .iter()
            .take(limit)
            .cloned()
            .collect()
    }

    /// Mark file as modified.
    pub async fn mark_modified(&self, path: &Path) {
        if let Some(state) = self.files.write().await.get_mut(path) {
            state.status = FileStatus::Modified;
            state.mark_dirty();
            state.modified_at = timestamp_now();
        }

        // Notify listeners
        self.notify_change(FileChange::Modified(path.to_path_buf()))
            .await;
    }

    /// Mark file as saved.
    pub async fn mark_saved(&self, path: &Path) {
        if let Some(state) = self.files.write().await.get_mut(path) {
            state.mark_clean();
        }
    }

    /// Record file creation.
    pub async fn record_created(&self, path: impl AsRef<Path>) {
        let path = path.as_ref().to_path_buf();
        let mut state = FileState::new(&path);
        state.status = FileStatus::Created;
        let _ = state.refresh();

        self.files.write().await.insert(path.clone(), state);
        self.notify_change(FileChange::Created(path)).await;
    }

    /// Record file deletion.
    pub async fn record_deleted(&self, path: &Path) {
        if let Some(state) = self.files.write().await.get_mut(path) {
            state.status = FileStatus::Deleted;
        }
        self.notify_change(FileChange::Deleted(path.to_path_buf()))
            .await;
    }

    /// List all tracked files.
    pub async fn list_files(&self) -> Vec<FileState> {
        self.files.read().await.values().cloned().collect()
    }

    /// List files by status.
    pub async fn list_by_status(&self, status: FileStatus) -> Vec<FileState> {
        self.files
            .read()
            .await
            .values()
            .filter(|f| f.status == status)
            .cloned()
            .collect()
    }

    /// List files by language.
    pub async fn list_by_language(&self, language: &str) -> Vec<FileState> {
        self.files
            .read()
            .await
            .values()
            .filter(|f| f.language.as_deref() == Some(language))
            .cloned()
            .collect()
    }

    /// Get dirty files.
    pub async fn dirty_files(&self) -> Vec<FileState> {
        self.files
            .read()
            .await
            .values()
            .filter(|f| f.is_dirty)
            .cloned()
            .collect()
    }

    /// Get file count.
    pub async fn file_count(&self) -> usize {
        self.files.read().await.len()
    }

    /// Add change listener.
    pub async fn add_listener(&self, listener: Arc<dyn FileChangeListener>) {
        self.change_listeners.write().await.push(listener);
    }

    /// Notify change listeners.
    async fn notify_change(&self, change: FileChange) {
        let listeners = self.change_listeners.read().await;
        for listener in listeners.iter() {
            listener.on_change(&change).await;
        }
    }

    /// Get workspace statistics.
    pub async fn stats(&self) -> WorkspaceStats {
        let files = self.files.read().await;

        let mut by_language: HashMap<String, u32> = HashMap::new();
        let mut total_size = 0u64;
        let mut binary_count = 0u32;

        for file in files.values() {
            total_size += file.size;

            if file.is_binary {
                binary_count += 1;
            }

            if let Some(ref lang) = file.language {
                *by_language.entry(lang.clone()).or_default() += 1;
            }
        }

        WorkspaceStats {
            total_files: files.len(),
            open_files: self.open_files.read().await.len(),
            dirty_files: files.values().filter(|f| f.is_dirty).count(),
            total_size,
            binary_files: binary_count,
            by_language,
        }
    }

    /// Refresh all file states.
    pub async fn refresh_all(&self) -> Result<u32> {
        let mut refreshed = 0u32;
        let mut files = self.files.write().await;

        for state in files.values_mut() {
            if state.refresh().is_ok() {
                refreshed += 1;
            }
        }

        Ok(refreshed)
    }

    /// Clear workspace.
    pub async fn clear(&self) {
        self.files.write().await.clear();
        self.open_files.write().await.clear();
        self.recent_files.write().await.clear();
    }
}

/// Scan result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanResult {
    /// Files found.
    pub files_found: usize,
    /// Directories found.
    pub dirs_found: usize,
    /// Total size in bytes.
    pub total_size: u64,
}

/// Workspace statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceStats {
    /// Total tracked files.
    pub total_files: usize,
    /// Open files.
    pub open_files: usize,
    /// Dirty files.
    pub dirty_files: usize,
    /// Total size.
    pub total_size: u64,
    /// Binary files.
    pub binary_files: u32,
    /// Files by language.
    pub by_language: HashMap<String, u32>,
}

/// File change event.
#[derive(Debug, Clone)]
pub enum FileChange {
    /// File created.
    Created(PathBuf),
    /// File modified.
    Modified(PathBuf),
    /// File deleted.
    Deleted(PathBuf),
    /// File renamed.
    Renamed { from: PathBuf, to: PathBuf },
}

/// File change listener.
#[async_trait::async_trait]
pub trait FileChangeListener: Send + Sync {
    /// Called when a file changes.
    async fn on_change(&self, change: &FileChange);
}

/// Detect file language from extension.
fn detect_language(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?;

    let lang = match ext.to_lowercase().as_str() {
        "rs" => "rust",
        "py" => "python",
        "js" => "javascript",
        "ts" => "typescript",
        "tsx" | "jsx" => "typescript",
        "go" => "go",
        "java" => "java",
        "rb" => "ruby",
        "php" => "php",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" => "cpp",
        "cs" => "csharp",
        "swift" => "swift",
        "kt" | "kts" => "kotlin",
        "scala" => "scala",
        "sh" | "bash" => "shell",
        "sql" => "sql",
        "html" | "htm" => "html",
        "css" | "scss" | "sass" | "less" => "css",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "xml" => "xml",
        "md" | "markdown" => "markdown",
        "txt" => "text",
        _ => return None,
    };

    Some(lang.to_string())
}

/// Check if file is binary.
fn is_binary_file(path: &Path) -> bool {
    const BINARY_EXTENSIONS: &[&str] = &[
        "exe", "dll", "so", "dylib", "a", "o", "obj", "pyc", "pyo", "png", "jpg", "jpeg", "gif",
        "bmp", "ico", "webp", "mp3", "mp4", "wav", "avi", "mov", "mkv", "zip", "tar", "gz", "bz2",
        "xz", "7z", "rar", "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "wasm", "class",
    ];

    if let Some(ext) = path.extension()
        && let Some(ext_str) = ext.to_str()
    {
        return BINARY_EXTENSIONS.contains(&ext_str.to_lowercase().as_str());
    }

    false
}

/// Get current timestamp.
fn timestamp_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_file_state() {
        let state = FileState::new("/test/file.rs");
        assert_eq!(state.status, FileStatus::Unknown);
        assert!(!state.is_open);
        assert!(!state.is_dirty);
    }

    #[test]
    fn test_detect_language() {
        assert_eq!(
            detect_language(Path::new("main.rs")),
            Some("rust".to_string())
        );
        assert_eq!(
            detect_language(Path::new("app.py")),
            Some("python".to_string())
        );
        assert_eq!(
            detect_language(Path::new("index.tsx")),
            Some("typescript".to_string())
        );
    }

    #[test]
    fn test_is_binary() {
        assert!(is_binary_file(Path::new("image.png")));
        assert!(is_binary_file(Path::new("app.exe")));
        assert!(!is_binary_file(Path::new("main.rs")));
    }

    #[tokio::test]
    async fn test_workspace() {
        let dir = tempdir().unwrap();
        let config = WorkspaceConfig {
            root: dir.path().to_path_buf(),
            ..Default::default()
        };

        let workspace = Workspace::new(config);

        // Create a test file
        let file_path = dir.path().join("test.rs");
        std::fs::write(&file_path, "fn main() {}").unwrap();

        let state = workspace.track_file(&file_path).await.unwrap();
        assert_eq!(state.language, Some("rust".to_string()));

        workspace.open_file(&file_path).await.unwrap();
        assert_eq!(workspace.open_files().await.len(), 1);
    }
}
