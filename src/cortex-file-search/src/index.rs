//! File index for storing and querying indexed files.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::cache::get_mtime;

/// Represents an indexed file entry.
#[derive(Debug, Clone)]
pub struct IndexedFile {
    /// Relative path from the search root.
    pub relative_path: PathBuf,

    /// Absolute path to the file.
    pub absolute_path: PathBuf,

    /// File name (last component of the path).
    pub file_name: String,

    /// File extension (without the dot).
    pub extension: Option<String>,

    /// File size in bytes.
    pub size: u64,

    /// File modification time (Unix timestamp).
    pub modified_time: Option<u64>,

    /// Directory depth from root.
    pub depth: usize,
}

impl IndexedFile {
    /// Creates a new indexed file entry.
    pub fn new(
        relative_path: PathBuf,
        absolute_path: PathBuf,
        size: u64,
        modified_time: Option<u64>,
    ) -> Self {
        let file_name = absolute_path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        let extension = absolute_path
            .extension()
            .map(|s| s.to_string_lossy().to_string());

        let depth = relative_path.components().count();

        Self {
            relative_path,
            absolute_path,
            file_name,
            extension,
            size,
            modified_time,
            depth,
        }
    }
}

/// Index of files for searching.
#[derive(Debug)]
pub struct FileIndex {
    /// Root directory of the index.
    root: PathBuf,

    /// All indexed files.
    files: Vec<IndexedFile>,

    /// File names for quick lookup.
    file_names: Vec<String>,

    /// Relative paths for quick lookup.
    relative_paths: Vec<String>,

    /// Index by extension.
    by_extension: HashMap<String, Vec<usize>>,

    /// Index by directory.
    by_directory: HashMap<PathBuf, Vec<usize>>,

    /// When the index was last built.
    built_at: Option<Instant>,

    /// Whether the index is currently being built.
    building: bool,

    /// Directories that have been modified since last build.
    dirty_dirs: HashSet<PathBuf>,
}

impl FileIndex {
    /// Creates a new empty file index.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            files: Vec::new(),
            file_names: Vec::new(),
            relative_paths: Vec::new(),
            by_extension: HashMap::new(),
            by_directory: HashMap::new(),
            built_at: None,
            building: false,
            dirty_dirs: HashSet::new(),
        }
    }

    /// Returns the root directory of this index.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns whether the index has been built.
    pub fn is_built(&self) -> bool {
        self.built_at.is_some()
    }

    /// Returns whether the index is currently being built.
    pub fn is_building(&self) -> bool {
        self.building
    }

    /// Sets the building state.
    pub fn set_building(&mut self, building: bool) {
        self.building = building;
    }

    /// Returns the number of indexed files.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Returns whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Returns when the index was last built.
    pub fn built_at(&self) -> Option<Instant> {
        self.built_at
    }

    /// Clears the index.
    pub fn clear(&mut self) {
        self.files.clear();
        self.file_names.clear();
        self.relative_paths.clear();
        self.by_extension.clear();
        self.by_directory.clear();
        self.built_at = None;
        self.dirty_dirs.clear();
    }

    /// Adds a file to the index.
    pub fn add_file(&mut self, file: IndexedFile) {
        let idx = self.files.len();

        // Add to extension index
        if let Some(ref ext) = file.extension {
            self.by_extension
                .entry(ext.to_lowercase())
                .or_default()
                .push(idx);
        }

        // Add to directory index
        if let Some(parent) = file.relative_path.parent() {
            self.by_directory
                .entry(parent.to_path_buf())
                .or_default()
                .push(idx);
        }

        // Store file name and path for fast search
        // Normalize path separators to forward slashes for consistent search
        self.file_names.push(file.file_name.clone());
        let normalized_path = file.relative_path.to_string_lossy().replace('\\', "/");
        self.relative_paths.push(normalized_path);

        self.files.push(file);
    }

    /// Marks the build as complete.
    pub fn mark_built(&mut self) {
        self.built_at = Some(Instant::now());
        self.building = false;
        self.dirty_dirs.clear();
    }

    /// Returns all indexed files.
    pub fn files(&self) -> &[IndexedFile] {
        &self.files
    }

    /// Returns file names for searching.
    pub fn file_names(&self) -> &[String] {
        &self.file_names
    }

    /// Returns relative paths for searching.
    pub fn relative_paths(&self) -> &[String] {
        &self.relative_paths
    }

    /// Gets files by extension.
    pub fn files_by_extension(&self, ext: &str) -> Vec<&IndexedFile> {
        self.by_extension
            .get(&ext.to_lowercase())
            .map(|indices| indices.iter().map(|&i| &self.files[i]).collect())
            .unwrap_or_default()
    }

    /// Gets files in a directory.
    pub fn files_in_directory(&self, dir: &Path) -> Vec<&IndexedFile> {
        self.by_directory
            .get(dir)
            .map(|indices| indices.iter().map(|&i| &self.files[i]).collect())
            .unwrap_or_default()
    }

    /// Gets a file by index.
    pub fn get(&self, index: usize) -> Option<&IndexedFile> {
        self.files.get(index)
    }

    /// Marks a directory as dirty (needs reindexing).
    pub fn mark_dirty(&mut self, dir: &Path) {
        self.dirty_dirs.insert(dir.to_path_buf());
    }

    /// Returns whether any directories are dirty.
    pub fn has_dirty_dirs(&self) -> bool {
        !self.dirty_dirs.is_empty()
    }

    /// Returns dirty directories.
    pub fn dirty_dirs(&self) -> &HashSet<PathBuf> {
        &self.dirty_dirs
    }

    /// Removes files from a specific directory (for incremental updates).
    pub fn remove_files_in_directory(&mut self, dir: &Path) {
        // Get indices to remove
        let indices_to_remove: HashSet<usize> = self
            .by_directory
            .get(dir)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();

        if indices_to_remove.is_empty() {
            return;
        }

        // Create new vectors without the removed files
        let mut new_files = Vec::new();
        let mut new_file_names = Vec::new();
        let mut new_relative_paths = Vec::new();
        let mut index_map: HashMap<usize, usize> = HashMap::new();

        for (old_idx, file) in self.files.iter().enumerate() {
            if !indices_to_remove.contains(&old_idx) {
                let new_idx = new_files.len();
                index_map.insert(old_idx, new_idx);
                new_files.push(file.clone());
                new_file_names.push(self.file_names[old_idx].clone());
                new_relative_paths.push(self.relative_paths[old_idx].clone());
            }
        }

        // Rebuild extension index
        let mut new_by_extension: HashMap<String, Vec<usize>> = HashMap::new();
        for (ext, indices) in &self.by_extension {
            let new_indices: Vec<usize> = indices
                .iter()
                .filter_map(|&i| index_map.get(&i).copied())
                .collect();
            if !new_indices.is_empty() {
                new_by_extension.insert(ext.clone(), new_indices);
            }
        }

        // Rebuild directory index
        let mut new_by_directory: HashMap<PathBuf, Vec<usize>> = HashMap::new();
        for (d, indices) in &self.by_directory {
            if d != dir {
                let new_indices: Vec<usize> = indices
                    .iter()
                    .filter_map(|&i| index_map.get(&i).copied())
                    .collect();
                if !new_indices.is_empty() {
                    new_by_directory.insert(d.clone(), new_indices);
                }
            }
        }

        self.files = new_files;
        self.file_names = new_file_names;
        self.relative_paths = new_relative_paths;
        self.by_extension = new_by_extension;
        self.by_directory = new_by_directory;
    }

    /// Checks if the index needs rebuilding based on root directory mtime.
    pub fn needs_rebuild(&self) -> bool {
        if self.built_at.is_none() {
            return true;
        }

        // Check if root directory has been modified
        if let Some(_current_mtime) = get_mtime(&self.root) {
            // Simple heuristic: if the index is older than the directory mtime
            // by more than a few seconds, consider rebuilding
            // This is a rough approximation since we're comparing Instant with SystemTime
            true // For now, always consider checking for updates
        } else {
            true
        }
    }

    /// Returns statistics about the index.
    pub fn stats(&self) -> IndexStats {
        let total_size: u64 = self.files.iter().map(|f| f.size).sum();
        let extensions: HashSet<_> = self
            .files
            .iter()
            .filter_map(|f| f.extension.clone())
            .collect();

        IndexStats {
            file_count: self.files.len(),
            total_size,
            extension_count: extensions.len(),
            directory_count: self.by_directory.len(),
        }
    }
}

/// Statistics about the file index.
#[derive(Debug, Clone)]
pub struct IndexStats {
    /// Total number of files indexed.
    pub file_count: usize,

    /// Total size of all indexed files.
    pub total_size: u64,

    /// Number of unique file extensions.
    pub extension_count: usize,

    /// Number of unique directories.
    pub directory_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_basic_operations() {
        let mut index = FileIndex::new("/test");

        assert!(!index.is_built());
        assert!(index.is_empty());

        let file = IndexedFile::new(
            PathBuf::from("src/main.rs"),
            PathBuf::from("/test/src/main.rs"),
            1024,
            Some(12345),
        );

        index.add_file(file);
        index.mark_built();

        assert!(index.is_built());
        assert_eq!(index.len(), 1);
    }

    #[test]
    fn test_index_by_extension() {
        let mut index = FileIndex::new("/test");

        index.add_file(IndexedFile::new(
            PathBuf::from("main.rs"),
            PathBuf::from("/test/main.rs"),
            100,
            None,
        ));
        index.add_file(IndexedFile::new(
            PathBuf::from("lib.rs"),
            PathBuf::from("/test/lib.rs"),
            200,
            None,
        ));
        index.add_file(IndexedFile::new(
            PathBuf::from("config.toml"),
            PathBuf::from("/test/config.toml"),
            50,
            None,
        ));

        let rs_files = index.files_by_extension("rs");
        assert_eq!(rs_files.len(), 2);

        let toml_files = index.files_by_extension("toml");
        assert_eq!(toml_files.len(), 1);
    }

    #[test]
    fn test_index_by_directory() {
        let mut index = FileIndex::new("/test");

        index.add_file(IndexedFile::new(
            PathBuf::from("src/main.rs"),
            PathBuf::from("/test/src/main.rs"),
            100,
            None,
        ));
        index.add_file(IndexedFile::new(
            PathBuf::from("src/lib.rs"),
            PathBuf::from("/test/src/lib.rs"),
            200,
            None,
        ));
        index.add_file(IndexedFile::new(
            PathBuf::from("tests/test.rs"),
            PathBuf::from("/test/tests/test.rs"),
            50,
            None,
        ));

        let src_files = index.files_in_directory(Path::new("src"));
        assert_eq!(src_files.len(), 2);

        let test_files = index.files_in_directory(Path::new("tests"));
        assert_eq!(test_files.len(), 1);
    }

    #[test]
    fn test_index_remove_directory() {
        let mut index = FileIndex::new("/test");

        index.add_file(IndexedFile::new(
            PathBuf::from("src/main.rs"),
            PathBuf::from("/test/src/main.rs"),
            100,
            None,
        ));
        index.add_file(IndexedFile::new(
            PathBuf::from("src/lib.rs"),
            PathBuf::from("/test/src/lib.rs"),
            200,
            None,
        ));
        index.add_file(IndexedFile::new(
            PathBuf::from("other/file.rs"),
            PathBuf::from("/test/other/file.rs"),
            50,
            None,
        ));

        assert_eq!(index.len(), 3);

        index.remove_files_in_directory(Path::new("src"));

        assert_eq!(index.len(), 1);
        assert_eq!(
            index.files()[0].relative_path,
            PathBuf::from("other/file.rs")
        );
    }

    #[test]
    fn test_indexed_file_properties() {
        let file = IndexedFile::new(
            PathBuf::from("deep/nested/path/file.rs"),
            PathBuf::from("/root/deep/nested/path/file.rs"),
            1024,
            Some(12345),
        );

        assert_eq!(file.file_name, "file.rs");
        assert_eq!(file.extension, Some("rs".to_string()));
        assert_eq!(file.depth, 4);
        assert_eq!(file.size, 1024);
    }
}
