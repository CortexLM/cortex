//! Edit file tool handler with cascading replacement strategies.
//!
//! This handler implements robust code editing using 8 cascading replacement
//! strategies to reliably find and replace text even when there are minor
//! differences in whitespace, indentation, or formatting.
//!
//! Strategies (in cascade order):
//! 1. SimpleReplacer - Exact string match
//! 2. LineTrimmedReplacer - Ignore spaces at start/end of each line
//! 3. BlockAnchorReplacer - Match by first and last line anchors
//! 4. WhitespaceNormalizedReplacer - Normalize all whitespace
//! 5. IndentationFlexibleReplacer - Ignore indentation differences
//! 6. EscapeNormalizedReplacer - Normalize escape characters
//! 7. TrimmedBoundaryReplacer - Match on trimmed content
//! 8. ContextAwareReplacer - Match using surrounding context
//!
//! File locking is used to prevent TOCTOU race conditions during read-modify-write.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::{info, warn};

use super::edit_strategies::{CascadeReplacer, EditError};
use super::{ToolContext, ToolHandler, ToolResult};
use crate::error::Result;
use crate::tools::spec::ToolMetadata;

/// Global lock manager for coordinating file edits within the process.
/// This prevents concurrent edits to the same file from different async tasks.
static EDIT_LOCKS: Lazy<
    Mutex<HashMap<std::path::PathBuf, std::sync::Arc<tokio::sync::Mutex<()>>>>,
> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Get or create a lock for a specific file path.
fn get_file_lock(path: &Path) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut locks = EDIT_LOCKS.lock().unwrap();
    locks
        .entry(canonical)
        .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

/// Perform an atomic file write using write-to-temp-then-rename pattern.
/// This ensures the file is never in a partially written state.
fn atomic_write_file(path: &Path, content: &str) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Cannot determine parent directory",
        )
    })?;

    // Create temp file in same directory for same-filesystem atomic rename
    let temp_path = parent.join(format!(
        ".{}.tmp.{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id()
    ));

    // Write to temp file with fsync
    {
        let mut temp_file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temp_path)?;

        temp_file.write_all(content.as_bytes())?;
        temp_file.sync_all()?;
    }

    // Atomic rename (on Unix) or replace (on Windows)
    #[cfg(unix)]
    {
        fs::rename(&temp_path, path).map_err(|e| {
            let _ = fs::remove_file(&temp_path);
            e
        })?;
    }

    #[cfg(windows)]
    {
        // On Windows, try to remove target first if exists
        if path.exists() {
            // Use a retry loop for Windows file replacement
            let mut retries = 3;
            loop {
                match fs::remove_file(path) {
                    Ok(()) => break,
                    Err(e) if retries > 0 => {
                        retries -= 1;
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    Err(e) => {
                        let _ = fs::remove_file(&temp_path);
                        return Err(e);
                    }
                }
            }
        }
        fs::rename(&temp_path, path).map_err(|e| {
            let _ = fs::remove_file(&temp_path);
            e
        })?;
    }

    Ok(())
}

/// Handler for Patch tool with fuzzy matching.
pub struct PatchHandler;

#[derive(Debug, Deserialize)]
struct PatchArgs {
    file_path: String,
    old_str: String,
    new_str: String,
    #[serde(default)]
    change_all: bool,
}

impl PatchHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PatchHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolHandler for PatchHandler {
    fn name(&self) -> &str {
        "Patch"
    }

    async fn execute(&self, arguments: Value, context: &ToolContext) -> Result<ToolResult> {
        let args: PatchArgs = serde_json::from_value(arguments)?;

        // Use validated path resolution to prevent path traversal attacks
        let path = match context.resolve_and_validate_path(&args.file_path) {
            Ok(p) => p,
            Err(e) => {
                return Ok(ToolResult::error(format!("Path validation failed: {}", e)));
            }
        };

        if !path.exists() {
            return Ok(ToolResult::error(format!(
                "File not found: {}",
                path.display()
            )));
        }

        // Acquire a lock for this file to prevent concurrent edits (TOCTOU protection)
        let file_lock = get_file_lock(&path);
        let _guard = file_lock.lock().await;

        // Read file content while holding the lock
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolResult::error(format!("Failed to read file: {e}")));
            }
        };

        // Use cascade replacer with 8 strategies
        let cascade = CascadeReplacer::new();

        let replace_result = if args.change_all {
            cascade.replace_all(&content, &args.old_str, &args.new_str)
        } else {
            cascade.replace(&content, &args.old_str, &args.new_str)
        };

        match replace_result {
            Ok(cascade_result) => {
                // Log the strategy that was used
                info!(
                    "Edit successful using strategy '{}' with {:.0}% confidence",
                    cascade_result.strategy_name,
                    cascade_result.confidence * 100.0
                );

                // Warn if we used a non-exact strategy
                if cascade_result.strategy_name != "SimpleReplacer" {
                    warn!(
                        "Used fuzzy matching strategy '{}' for edit in {}",
                        cascade_result.strategy_name,
                        path.display()
                    );
                }

                // Write the new content atomically (write to temp, then rename)
                // This prevents partial writes and ensures readers always see complete content
                match atomic_write_file(&path, &cascade_result.content) {
                    Ok(_) => {
                        let filename = path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("")
                            .to_string();
                        let extension = path
                            .extension()
                            .and_then(|e| e.to_str())
                            .unwrap_or("")
                            .to_string();

                        // Count actual replacements for metadata
                        let occurrences_replaced = if args.change_all {
                            content.matches(&args.old_str).count().max(1)
                        } else {
                            1
                        };

                        let metadata = ToolMetadata {
                            duration_ms: 0,
                            exit_code: Some(0),
                            files_modified: vec![args.file_path.clone()],
                            data: Some(json!({
                                "path": args.file_path,
                                "filename": filename,
                                "extension": extension,
                                "old_str": args.old_str,
                                "new_str": args.new_str,
                                "match_strategy": cascade_result.strategy_name,
                                "match_confidence": cascade_result.confidence,
                                "occurrences_replaced": occurrences_replaced
                            })),
                        };

                        let message = if cascade_result.strategy_name == "SimpleReplacer" {
                            format!("Successfully edited {}", path.display())
                        } else {
                            format!(
                                "Successfully edited {} (using {} with {:.0}% confidence)",
                                path.display(),
                                cascade_result.strategy_name,
                                cascade_result.confidence * 100.0
                            )
                        };

                        Ok(ToolResult::success(message).with_metadata(metadata))
                    }
                    Err(e) => Ok(ToolResult::error(format!("Failed to write file: {e}"))),
                }
            }
            Err(EditError::NoMatchFound {
                search,
                strategies_tried,
            }) => {
                // Provide helpful error with all tried strategies
                let strategies_str = strategies_tried.join(", ");
                Ok(ToolResult::error(format!(
                    "Could not find '{}' in file {} (tried strategies: {})",
                    truncate_for_display(&search, 50),
                    path.display(),
                    strategies_str
                )))
            }
            Err(EditError::MultipleMatches {
                count,
                strategy,
                hint,
            }) => Ok(ToolResult::error(format!(
                "Found {} occurrences using {} strategy. {}",
                count, strategy, hint
            ))),
        }
    }
}

/// Provides diagnostic information about which strategies would match
#[allow(dead_code)]
pub fn diagnose_match(content: &str, search: &str) -> Vec<(&'static str, bool, f64)> {
    use super::edit_strategies::{
        BlockAnchorReplacer, ContextAwareReplacer, EditStrategy, EscapeNormalizedReplacer,
        IndentationFlexibleReplacer, LineTrimmedReplacer, SimpleReplacer, TrimmedBoundaryReplacer,
        WhitespaceNormalizedReplacer,
    };

    let strategies: Vec<Box<dyn EditStrategy>> = vec![
        Box::new(SimpleReplacer),
        Box::new(LineTrimmedReplacer),
        Box::new(BlockAnchorReplacer),
        Box::new(WhitespaceNormalizedReplacer),
        Box::new(IndentationFlexibleReplacer),
        Box::new(EscapeNormalizedReplacer),
        Box::new(TrimmedBoundaryReplacer),
        Box::new(ContextAwareReplacer::default()),
    ];

    let mut diagnostics = Vec::new();

    for strategy in strategies {
        let would_match = strategy.try_replace(content, search, "__TEST__").is_some();
        diagnostics.push((strategy.name(), would_match, strategy.confidence()));
    }

    diagnostics
}

/// Truncate string for display in error messages
fn truncate_for_display(s: &str, max_len: usize) -> String {
    cortex_common::truncate_for_display(s, max_len).into_owned()
}
