//! File explorer endpoints.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Query, State},
};

use crate::error::{AppError, AppResult};
use crate::state::AppState;

use super::path_security::{validate_path_for_delete, validate_path_for_write};
use super::types::{
    CreateDirRequest, DeleteFileRequest, DeleteFileResponse, FileEntry, FileTreeNode,
    FileTreeQuery, ListFilesRequest, ListFilesResponse, ReadFileRequest, ReadFileResponse,
    RenameRequest, WriteFileRequest, WriteFileResponse,
};

/// Maximum depth limit for file tree to prevent stack overflow and DoS.
const MAX_TREE_DEPTH: usize = 20;

/// Maximum number of entries per directory to prevent memory exhaustion.
const MAX_ENTRIES_PER_DIR: usize = 1000;

/// List files in a directory.
pub async fn list_files(Json(req): Json<ListFilesRequest>) -> AppResult<Json<ListFilesResponse>> {
    use std::fs;

    let path = std::path::Path::new(&req.path);

    if !path.exists() {
        return Err(AppError::NotFound(format!("Path not found: {}", req.path)));
    }

    if !path.is_dir() {
        return Err(AppError::BadRequest("Path is not a directory".to_string()));
    }

    let mut entries = Vec::new();

    let dir_entries = fs::read_dir(path)
        .map_err(|e| AppError::Internal(format!("Failed to read directory: {e}")))?;

    for entry in dir_entries {
        let entry = entry.map_err(|e| AppError::Internal(format!("Failed to read entry: {e}")))?;
        let metadata = entry
            .metadata()
            .map_err(|e| AppError::Internal(format!("Failed to read metadata: {e}")))?;

        let name = entry.file_name().to_string_lossy().to_string();
        let entry_path = entry.path().to_string_lossy().to_string();

        let file_type = if metadata.is_dir() {
            "directory"
        } else if metadata.file_type().is_symlink() {
            "symlink"
        } else {
            "file"
        };

        let modified = metadata.modified().ok().map(|t| {
            let datetime: chrono::DateTime<chrono::Utc> = t.into();
            datetime.format("%Y-%m-%dT%H:%M:%S").to_string()
        });

        entries.push(FileEntry {
            name,
            path: entry_path,
            file_type: file_type.to_string(),
            size: metadata.len(),
            modified,
        });
    }

    // Sort: directories first, then alphabetically
    entries.sort_by(|a, b| match (&a.file_type[..], &b.file_type[..]) {
        ("directory", "file") | ("directory", "symlink") => std::cmp::Ordering::Less,
        ("file", "directory") | ("symlink", "directory") => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    Ok(Json(ListFilesResponse {
        path: req.path,
        entries,
    }))
}

/// Get file tree with depth limiting and safeguards.
pub async fn get_file_tree(Query(query): Query<FileTreeQuery>) -> AppResult<Json<FileTreeNode>> {
    use std::fs;

    // Enforce maximum depth limit to prevent stack overflow
    let max_depth = query.depth.min(MAX_TREE_DEPTH);

    /// Build file tree iteratively to avoid stack overflow on deep structures.
    fn build_tree(path: &std::path::Path, depth: usize, max_depth: usize) -> Option<FileTreeNode> {
        // Hard safety check
        if depth > MAX_TREE_DEPTH {
            return None;
        }

        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());

        let is_dir = path.is_dir();
        let mut truncated = None;

        let children = if is_dir && depth < max_depth {
            match fs::read_dir(path) {
                Ok(entries) => {
                    let filtered_entries: Vec<_> = entries
                        .filter_map(|e| e.ok())
                        .filter(|entry| {
                            let entry_name = entry.file_name().to_string_lossy().to_string();
                            // Skip hidden files and common ignored directories
                            !entry_name.starts_with('.')
                                && !matches!(
                                    entry_name.as_str(),
                                    "node_modules"
                                        | "target"
                                        | "dist"
                                        | "build"
                                        | "__pycache__"
                                        | ".git"
                                        | "venv"
                                        | ".venv"
                                )
                        })
                        .take(MAX_ENTRIES_PER_DIR + 1) // Take one extra to detect truncation
                        .collect();

                    // Check if we hit the limit
                    let was_truncated = filtered_entries.len() > MAX_ENTRIES_PER_DIR;
                    if was_truncated {
                        truncated = Some(true);
                    }

                    let mut children: Vec<FileTreeNode> = filtered_entries
                        .into_iter()
                        .take(MAX_ENTRIES_PER_DIR)
                        .filter_map(|entry| {
                            let entry_path = entry.path();
                            build_tree(&entry_path, depth + 1, max_depth)
                        })
                        .collect();

                    // Sort: folders first, then files, alphabetically
                    children.sort_by(|a, b| match (a.is_dir, b.is_dir) {
                        (true, false) => std::cmp::Ordering::Less,
                        (false, true) => std::cmp::Ordering::Greater,
                        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                    });

                    Some(children)
                }
                Err(_) => None,
            }
        } else if is_dir {
            // Directory but max depth reached - indicate it has potential children
            Some(vec![])
        } else {
            None
        };

        Some(FileTreeNode {
            name,
            path: path.to_string_lossy().to_string(),
            is_dir,
            children,
            truncated,
        })
    }

    let path = std::path::Path::new(&query.path);

    if !path.exists() {
        return Err(AppError::NotFound(format!(
            "Path not found: {}",
            query.path
        )));
    }

    let tree = build_tree(path, 0, max_depth)
        .ok_or_else(|| AppError::Internal("Failed to build file tree".to_string()))?;

    Ok(Json(tree))
}

/// Read file content.
pub async fn read_file(Json(req): Json<ReadFileRequest>) -> AppResult<Json<ReadFileResponse>> {
    use std::fs;

    let path = std::path::Path::new(&req.path);

    if !path.exists() {
        return Err(AppError::NotFound(format!("File not found: {}", req.path)));
    }

    if !path.is_file() {
        return Err(AppError::BadRequest("Path is not a file".to_string()));
    }

    let content = fs::read_to_string(path)
        .map_err(|e| AppError::Internal(format!("Failed to read file: {e}")))?;

    let size = path.metadata().map(|m| m.len()).unwrap_or(0);

    Ok(Json(ReadFileResponse {
        path: req.path,
        content,
        size,
    }))
}

/// Write file content.
pub async fn write_file(Json(req): Json<WriteFileRequest>) -> AppResult<Json<WriteFileResponse>> {
    use std::fs;

    let path = std::path::Path::new(&req.path);

    // Validate path to prevent traversal attacks
    let validated_path = validate_path_for_write(path).map_err(AppError::BadRequest)?;

    // Create parent directories if needed
    if let Some(parent) = validated_path.parent()
        && !parent.exists()
    {
        fs::create_dir_all(parent)
            .map_err(|e| AppError::Internal(format!("Failed to create directory: {e}")))?;
    }

    fs::write(&validated_path, &req.content)
        .map_err(|e| AppError::Internal(format!("Failed to write file: {e}")))?;

    Ok(Json(WriteFileResponse {
        path: req.path.clone(),
        size: req.content.len() as u64,
        success: true,
    }))
}

/// Delete file or directory.
pub async fn delete_file(
    Json(req): Json<DeleteFileRequest>,
) -> AppResult<Json<DeleteFileResponse>> {
    use std::fs;

    let path = std::path::Path::new(&req.path);

    // Validate path to prevent traversal attacks
    let validated_path = validate_path_for_delete(path).map_err(AppError::BadRequest)?;

    if !validated_path.exists() {
        return Err(AppError::NotFound(format!("Path not found: {}", req.path)));
    }

    if validated_path.is_dir() {
        fs::remove_dir_all(&validated_path)
            .map_err(|e| AppError::Internal(format!("Failed to delete directory: {e}")))?;
    } else {
        fs::remove_file(&validated_path)
            .map_err(|e| AppError::Internal(format!("Failed to delete file: {e}")))?;
    }

    Ok(Json(DeleteFileResponse {
        path: req.path,
        success: true,
    }))
}

/// Create directory.
pub async fn create_directory(
    Json(req): Json<CreateDirRequest>,
) -> AppResult<Json<DeleteFileResponse>> {
    use std::fs;

    let path = std::path::Path::new(&req.path);

    fs::create_dir_all(path)
        .map_err(|e| AppError::Internal(format!("Failed to create directory: {e}")))?;

    Ok(Json(DeleteFileResponse {
        path: req.path,
        success: true,
    }))
}

/// Rename file or directory.
pub async fn rename_file(Json(req): Json<RenameRequest>) -> AppResult<Json<DeleteFileResponse>> {
    use std::fs;

    let old_path = std::path::Path::new(&req.old_path);

    if !old_path.exists() {
        return Err(AppError::NotFound(format!(
            "Path not found: {}",
            req.old_path
        )));
    }

    fs::rename(&req.old_path, &req.new_path)
        .map_err(|e| AppError::Internal(format!("Failed to rename: {e}")))?;

    Ok(Json(DeleteFileResponse {
        path: req.new_path,
        success: true,
    }))
}

/// Watch for file changes via Server-Sent Events.
pub async fn watch_files(
    State(state): State<Arc<AppState>>,
) -> axum::response::Sse<
    impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
> {
    use axum::response::sse::{Event, KeepAlive};

    let mut rx = state.subscribe_file_changes();

    let stream = async_stream::stream! {
        // Send initial connected event
        yield Ok(Event::default().event("connected").data("File watcher connected"));

        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Ok(json) = serde_json::to_string(&event) {
                        yield Ok(Event::default().event("file_change").data(json));
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    // Skip lagged events
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    };

    axum::response::Sse::new(stream).keep_alive(KeepAlive::default())
}
