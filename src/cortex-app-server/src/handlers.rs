//! Request handlers for the API.
//!
//! This module contains supplementary request handlers. The primary API handlers
//! are in `api.rs`. These handlers are used for specialized endpoints or as
//! building blocks for other functionality.

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State},
    response::Sse,
};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::tools::ToolExecutor;

/// Streaming completion handler.
/// NOTE: This endpoint is temporarily disabled as the providers module was removed.
/// Use the TUI (cargo run --bin cortex) for LLM completions instead.
pub async fn stream_completion(
    State(_state): State<Arc<AppState>>,
    Json(_req): Json<StreamCompletionRequest>,
) -> AppResult<Sse<impl Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>>>
{
    // Return a stream with a single error message
    let stream = async_stream::stream! {
        let error_event = StreamEvent::Error {
            message: "Streaming completions API endpoint is temporarily disabled. The providers module was removed during dead code cleanup. Please use the TUI (cargo run --bin cortex) for LLM interactions.".to_string()
        };
        let data = serde_json::to_string(&error_event).unwrap_or_default();
        yield Ok(axum::response::sse::Event::default().data(data));
    };

    Ok(Sse::new(stream))
}

/// Stream completion request.
#[derive(Debug, Deserialize)]
pub struct StreamCompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

/// Message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

/// Streaming event.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// Text delta.
    Delta { content: String },
    /// Tool call.
    ToolCall {
        id: String,
        name: String,
        arguments: String,
    },
    /// Done event.
    Done { usage: TokenUsage },
    /// Error event.
    Error { message: String },
}

/// Token usage.
#[derive(Debug, Clone, Serialize)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// Execute a command handler using the secure ToolExecutor.
pub async fn execute_command(
    State(_state): State<Arc<AppState>>,
    Json(req): Json<ExecuteCommandRequest>,
) -> AppResult<Json<ExecuteCommandResponse>> {
    let cwd =
        req.workdir.as_ref().map(PathBuf::from).unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/workspace"))
        });

    let mut executor = ToolExecutor::new(cwd);

    if let Some(timeout) = req.timeout {
        executor = executor.with_timeout(timeout);
    }

    // Use the secure tool executor which prevents command injection
    let result = executor
        .execute(
            "Execute",
            serde_json::json!({
                "command": req.command,
            }),
        )
        .await;

    let exit_code = result
        .metadata
        .as_ref()
        .and_then(|m| m.get("exit_code"))
        .and_then(|v| v.as_i64())
        .map(|c| c as i32)
        .unwrap_or(if result.success { 0 } else { 1 });

    Ok(Json(ExecuteCommandResponse {
        id: Uuid::new_v4().to_string(),
        success: result.success,
        output: result.output,
        exit_code,
        stderr: result.error,
    }))
}

/// Execute command request.
#[derive(Debug, Deserialize)]
pub struct ExecuteCommandRequest {
    pub command: String,
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub env: Option<std::collections::HashMap<String, String>>,
}

/// Execute command response.
#[derive(Debug, Serialize)]
pub struct ExecuteCommandResponse {
    pub id: String,
    pub success: bool,
    pub output: String,
    pub exit_code: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
}

/// File operations handler - reads file content securely.
pub async fn read_file(
    State(_state): State<Arc<AppState>>,
    Path(path): Path<String>,
) -> AppResult<Json<FileContent>> {
    use tokio::fs;

    // Security: Validate path to prevent traversal attacks
    let file_path = PathBuf::from(&path);

    // Check for path traversal attempts
    if path.contains("..") {
        return Err(AppError::Authorization(
            "Path traversal not allowed".to_string(),
        ));
    }

    // Canonicalize to resolve any symlinks and ensure path is valid
    let canonical_path = file_path
        .canonicalize()
        .map_err(|e| AppError::NotFound(format!("File not found: {}", e)))?;

    // Read file metadata
    let metadata = fs::metadata(&canonical_path)
        .await
        .map_err(|e| AppError::NotFound(format!("Cannot access file: {}", e)))?;

    if !metadata.is_file() {
        return Err(AppError::BadRequest("Path is not a file".to_string()));
    }

    // Check file size limit (10MB)
    const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;
    if metadata.len() > MAX_FILE_SIZE {
        return Err(AppError::PayloadTooLarge);
    }

    // Read file content
    let content = fs::read_to_string(&canonical_path)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to read file: {}", e)))?;

    Ok(Json(FileContent {
        path,
        content,
        encoding: "utf-8".to_string(),
        size: metadata.len(),
    }))
}

/// File content response.
#[derive(Debug, Serialize)]
pub struct FileContent {
    pub path: String,
    pub content: String,
    pub encoding: String,
    pub size: u64,
}

/// Write file request.
#[derive(Debug, Deserialize)]
pub struct WriteFileRequest {
    pub content: String,
    #[serde(default)]
    pub encoding: Option<String>,
    #[serde(default)]
    pub create_dirs: bool,
}

/// Write file handler with proper implementation.
pub async fn write_file(
    State(_state): State<Arc<AppState>>,
    Path(path): Path<String>,
    Json(req): Json<WriteFileRequest>,
) -> AppResult<Json<WriteFileResponse>> {
    use tokio::fs;

    // Security: Validate path to prevent traversal attacks
    if path.contains("..") {
        return Err(AppError::Authorization(
            "Path traversal not allowed".to_string(),
        ));
    }

    let file_path = PathBuf::from(&path);

    // Create parent directories if requested and needed
    if req.create_dirs
        && let Some(parent) = file_path.parent()
    {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| AppError::Internal(format!("Failed to create directories: {}", e)))?;
    }

    // Write file content
    fs::write(&file_path, &req.content)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to write file: {}", e)))?;

    Ok(Json(WriteFileResponse {
        path,
        bytes_written: req.content.len() as u64,
        success: true,
    }))
}

/// Write file response.
#[derive(Debug, Serialize)]
pub struct WriteFileResponse {
    pub path: String,
    pub bytes_written: u64,
    pub success: bool,
}

/// List directory handler with real implementation.
pub async fn list_directory(
    State(_state): State<Arc<AppState>>,
    Path(path): Path<String>,
    Query(query): Query<ListDirQuery>,
) -> AppResult<Json<DirectoryListing>> {
    use tokio::fs;

    // Security: Validate path to prevent traversal attacks
    if path.contains("..") {
        return Err(AppError::Authorization(
            "Path traversal not allowed".to_string(),
        ));
    }

    let dir_path = PathBuf::from(&path);

    // Check if path exists and is a directory
    let metadata = fs::metadata(&dir_path)
        .await
        .map_err(|e| AppError::NotFound(format!("Directory not found: {}", e)))?;

    if !metadata.is_dir() {
        return Err(AppError::BadRequest("Path is not a directory".to_string()));
    }

    let mut entries = Vec::new();
    let mut dir_reader = fs::read_dir(&dir_path)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to read directory: {}", e)))?;

    while let Ok(Some(entry)) = dir_reader.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();

        // Skip hidden files unless requested
        if !query.include_hidden && name.starts_with('.') {
            continue;
        }

        // Apply pattern filter if specified
        if let Some(pattern) = &query.pattern
            && !name.contains(pattern)
        {
            continue;
        }

        let entry_metadata = entry.metadata().await.ok();
        let entry_type = entry_metadata
            .as_ref()
            .map(|m| {
                if m.is_dir() {
                    EntryType::Directory
                } else if m.file_type().is_symlink() {
                    EntryType::Symlink
                } else {
                    EntryType::File
                }
            })
            .unwrap_or(EntryType::File);

        let size = entry_metadata
            .as_ref()
            .filter(|m| m.is_file())
            .map(|m| m.len());

        let modified = entry_metadata
            .as_ref()
            .and_then(|m| m.modified().ok())
            .map(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            });

        entries.push(DirectoryEntry {
            name,
            entry_type,
            size,
            modified,
        });
    }

    // Sort: directories first, then alphabetically
    entries.sort_by(|a, b| match (&a.entry_type, &b.entry_type) {
        (EntryType::Directory, EntryType::File) | (EntryType::Directory, EntryType::Symlink) => {
            std::cmp::Ordering::Less
        }
        (EntryType::File, EntryType::Directory) | (EntryType::Symlink, EntryType::Directory) => {
            std::cmp::Ordering::Greater
        }
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    let total = entries.len();

    Ok(Json(DirectoryListing {
        path,
        entries,
        total,
    }))
}

/// List directory query.
#[derive(Debug, Deserialize)]
pub struct ListDirQuery {
    #[serde(default)]
    pub recursive: bool,
    #[serde(default)]
    pub include_hidden: bool,
    #[serde(default)]
    pub pattern: Option<String>,
}

/// Directory listing response.
#[derive(Debug, Serialize)]
pub struct DirectoryListing {
    pub path: String,
    pub entries: Vec<DirectoryEntry>,
    pub total: usize,
}

/// Directory entry.
#[derive(Debug, Serialize)]
pub struct DirectoryEntry {
    pub name: String,
    #[serde(rename = "type")]
    pub entry_type: EntryType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<u64>,
}

/// Entry type.
#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryType {
    File,
    Directory,
    Symlink,
}

/// Search files handler.
pub async fn search_files(
    State(_state): State<Arc<AppState>>,
    Json(req): Json<SearchFilesRequest>,
) -> AppResult<Json<SearchResults>> {
    Ok(Json(SearchResults {
        query: req.pattern.clone(),
        matches: vec![SearchMatch {
            path: "src/main.rs".to_string(),
            line: Some(10),
            content: Some(format!("Line containing {}", req.pattern)),
        }],
        total: 1,
    }))
}

/// Search files request.
#[derive(Debug, Deserialize)]
pub struct SearchFilesRequest {
    pub pattern: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub file_pattern: Option<String>,
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    100
}

/// Search results.
#[derive(Debug, Serialize)]
pub struct SearchResults {
    pub query: String,
    pub matches: Vec<SearchMatch>,
    pub total: usize,
}

/// Search match.
#[derive(Debug, Serialize)]
pub struct SearchMatch {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// Git operations handler.
pub async fn git_status(
    State(_state): State<Arc<AppState>>,
    Path(_repo): Path<String>,
) -> AppResult<Json<GitStatus>> {
    Ok(Json(GitStatus {
        branch: "main".to_string(),
        ahead: 0,
        behind: 0,
        staged: vec![],
        modified: vec!["src/main.rs".to_string()],
        untracked: vec!["new_file.txt".to_string()],
    }))
}

/// Git status response.
#[derive(Debug, Serialize)]
pub struct GitStatus {
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub staged: Vec<String>,
    pub modified: Vec<String>,
    pub untracked: Vec<String>,
}

/// Git diff handler.
pub async fn git_diff(
    State(_state): State<Arc<AppState>>,
    Path(_repo): Path<String>,
    Query(_query): Query<GitDiffQuery>,
) -> AppResult<Json<GitDiff>> {
    Ok(Json(GitDiff {
        files: vec![FileDiff {
            path: "src/main.rs".to_string(),
            status: "modified".to_string(),
            additions: 5,
            deletions: 2,
            patch: Some("@@ -1,5 +1,8 @@\n+// New comment\n fn main() {\n".to_string()),
        }],
        stats: DiffStats {
            files_changed: 1,
            insertions: 5,
            deletions: 2,
        },
    }))
}

/// Git diff query.
#[derive(Debug, Deserialize)]
pub struct GitDiffQuery {
    #[serde(default)]
    pub staged: bool,
    #[serde(default)]
    pub commit: Option<String>,
}

/// Git diff response.
#[derive(Debug, Serialize)]
pub struct GitDiff {
    pub files: Vec<FileDiff>,
    pub stats: DiffStats,
}

/// File diff.
#[derive(Debug, Serialize)]
pub struct FileDiff {
    pub path: String,
    pub status: String,
    pub additions: u32,
    pub deletions: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
}

/// Diff statistics.
#[derive(Debug, Serialize)]
pub struct DiffStats {
    pub files_changed: u32,
    pub insertions: u32,
    pub deletions: u32,
}

/// Process information handler.
pub async fn list_processes(State(_state): State<Arc<AppState>>) -> AppResult<Json<ProcessList>> {
    Ok(Json(ProcessList {
        processes: vec![ProcessInfo {
            pid: 1234,
            name: "cortex-server".to_string(),
            cpu: 2.5,
            memory: 128 * 1024 * 1024,
            status: "running".to_string(),
        }],
    }))
}

/// Process list.
#[derive(Debug, Serialize)]
pub struct ProcessList {
    pub processes: Vec<ProcessInfo>,
}

/// Process information.
#[derive(Debug, Serialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub cpu: f32,
    pub memory: u64,
    pub status: String,
}

/// Environment info handler.
pub async fn get_environment(State(_state): State<Arc<AppState>>) -> Json<EnvironmentInfo> {
    Json(EnvironmentInfo {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        cwd: std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
        home: std::env::var("HOME").unwrap_or_default(),
        shell: std::env::var("SHELL").unwrap_or_default(),
        user: std::env::var("USER").unwrap_or_default(),
    })
}

/// Environment information.
#[derive(Debug, Serialize)]
pub struct EnvironmentInfo {
    pub os: String,
    pub arch: String,
    pub cwd: String,
    pub home: String,
    pub shell: String,
    pub user: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_event_serialization() {
        let event = StreamEvent::Delta {
            content: "Hello".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("delta"));
        assert!(json.contains("Hello"));
    }

    #[test]
    fn test_token_usage() {
        let usage = TokenUsage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
        };
        assert_eq!(
            usage.total_tokens,
            usage.prompt_tokens + usage.completion_tokens
        );
    }
}
