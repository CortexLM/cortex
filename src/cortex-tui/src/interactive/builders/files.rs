//! Builders for file-related interactive selections.

use crate::interactive::state::{InteractiveAction, InteractiveItem, InteractiveState};
use std::path::{Path, PathBuf};

/// Build an interactive state showing current context files.
pub fn build_context_list(context_files: &[PathBuf]) -> InteractiveState {
    let items: Vec<InteractiveItem> = context_files
        .iter()
        .enumerate()
        .map(|(idx, path)| {
            let label = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();

            let description = path
                .parent()
                .and_then(|p| p.to_str())
                .map(|s| s.to_string());

            let icon = get_file_icon(path);

            let mut item = InteractiveItem::new(idx.to_string(), label)
                .with_icon(icon)
                .with_path(path.clone());

            if let Some(desc) = description {
                item = item.with_description(desc);
            }

            item
        })
        .collect();

    if items.is_empty() {
        let empty_item = InteractiveItem::new("empty", "No files in context")
            .with_description("Use /add to add files")
            .with_disabled(true);

        InteractiveState::new(
            "Context Files",
            vec![empty_item],
            InteractiveAction::Custom("context".into()),
        )
    } else {
        InteractiveState::new(
            format!("Context Files ({})", items.len()),
            items,
            InteractiveAction::Custom("context".into()),
        )
    }
}

/// Build an interactive state for removing context files (multi-select).
pub fn build_context_remove(context_files: &[PathBuf]) -> InteractiveState {
    let items: Vec<InteractiveItem> = context_files
        .iter()
        .enumerate()
        .map(|(idx, path)| {
            let label = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();

            let description = path.to_str().map(|s| s.to_string());
            let icon = get_file_icon(path);

            let mut item = InteractiveItem::new(idx.to_string(), label)
                .with_icon(icon)
                .with_path(path.clone());

            if let Some(desc) = description {
                item = item.with_description(desc);
            }

            item
        })
        .collect();

    if items.is_empty() {
        let empty_item = InteractiveItem::new("empty", "No files to remove").with_disabled(true);

        InteractiveState::new(
            "Remove Files",
            vec![empty_item],
            InteractiveAction::RemoveContextFiles,
        )
    } else {
        InteractiveState::new(
            format!("Remove Files ({})", items.len()),
            items,
            InteractiveAction::RemoveContextFiles,
        )
        .with_multi_select()
    }
}

/// Build a file browser for adding files.
pub fn build_file_browser(base_path: &Path) -> InteractiveState {
    let mut items = Vec::new();

    // Add parent directory option if not at root
    if let Some(parent) = base_path.parent() {
        items.push(
            InteractiveItem::new("..", "..")
                .with_icon('<')
                .with_description("Parent directory")
                .with_path(parent.to_path_buf()),
        );
    }

    // Read directory contents
    if let Ok(entries) = std::fs::read_dir(base_path) {
        let mut dirs = Vec::new();
        let mut files = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            // Skip hidden files
            if name.starts_with('.') {
                continue;
            }

            if path.is_dir() {
                dirs.push((name, path));
            } else {
                files.push((name, path));
            }
        }

        // Sort alphabetically
        dirs.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
        files.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

        // Add directories first
        for (name, path) in dirs {
            items.push(
                InteractiveItem::new(&name, format!("{}/", name))
                    .with_icon('/')
                    .with_path(path),
            );
        }

        // Then files
        for (name, path) in files {
            let icon = get_file_icon(&path);
            items.push(
                InteractiveItem::new(&name, &name)
                    .with_icon(icon)
                    .with_path(path),
            );
        }
    }

    if items.is_empty() {
        items.push(InteractiveItem::new("empty", "(empty directory)").with_disabled(true));
    }

    let title = format!("Browse: {}", base_path.display());

    InteractiveState::new(
        title,
        items,
        InteractiveAction::BrowseFiles {
            base_path: base_path.to_path_buf(),
        },
    )
    .with_search()
    .with_multi_select()
    .with_max_visible(15)
}

/// Get an icon for a file based on its extension.
fn get_file_icon(path: &Path) -> char {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());

    match ext.as_deref() {
        Some("rs") => 'R',
        Some("py") => 'P',
        Some("js" | "ts" | "jsx" | "tsx") => 'J',
        Some("json" | "yaml" | "yml" | "toml") => 'C',
        Some("md" | "txt" | "doc" | "docx") => 'T',
        Some("png" | "jpg" | "jpeg" | "gif" | "svg" | "webp") => 'I',
        Some("html" | "css" | "scss") => 'W',
        Some("sh" | "bash" | "zsh" | "fish") => '$',
        Some("sql") => 'D',
        Some("xml") => 'X',
        Some("go") => 'G',
        Some("java" | "kt" | "kts") => 'J',
        Some("c" | "cpp" | "h" | "hpp") => 'C',
        Some("rb") => 'R',
        Some("lock") => 'L',
        _ => '-',
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_context_list_empty() {
        let state = build_context_list(&[]);
        assert_eq!(state.items.len(), 1);
        assert!(state.items[0].disabled);
    }

    #[test]
    fn test_build_context_list_with_files() {
        let files = vec![
            PathBuf::from("/tmp/test.rs"),
            PathBuf::from("/tmp/other.py"),
        ];
        let state = build_context_list(&files);
        assert_eq!(state.items.len(), 2);
    }

    #[test]
    fn test_get_file_icon() {
        assert_eq!(get_file_icon(Path::new("test.rs")), 'R');
        assert_eq!(get_file_icon(Path::new("test.py")), 'P');
        assert_eq!(get_file_icon(Path::new("test.unknown")), '-');
    }
}
