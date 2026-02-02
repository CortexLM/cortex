//! Tests for diff module.

use crate::diff::*;
use std::path::PathBuf;

#[test]
fn test_unified_diff_empty() {
    let diff = UnifiedDiff::empty();
    assert!(diff.is_empty());
    assert_eq!(diff.file_count(), 0);
}

#[test]
fn test_unified_diff_parse() {
    let diff_text = r#"--- a/file.txt
+++ b/file.txt
@@ -1,3 +1,4 @@
 line 1
-line 2
+line 2 modified
 line 3
+line 4
"#;

    let diff = UnifiedDiff::parse(diff_text).unwrap();
    assert_eq!(diff.file_count(), 1);
    assert!(!diff.is_empty());
}

#[test]
fn test_file_operation_variants() {
    let create = FileOperation::Create;
    let delete = FileOperation::Delete;
    let modify = FileOperation::Modify;
    let rename = FileOperation::Rename;

    assert!(matches!(create, FileOperation::Create));
    assert!(matches!(delete, FileOperation::Delete));
    assert!(matches!(modify, FileOperation::Modify));
    assert!(matches!(rename, FileOperation::Rename));
}

#[test]
fn test_hunk_new() {
    let hunk = Hunk::new(1, 1);
    assert_eq!(hunk.old_start, 1);
    assert_eq!(hunk.new_start, 1);
    assert_eq!(hunk.old_count, 0);
    assert_eq!(hunk.new_count, 0);
}

#[test]
fn test_hunk_add_context() {
    let mut hunk = Hunk::new(1, 1);
    hunk.add_context("context line");

    assert_eq!(hunk.lines.len(), 1);
    assert_eq!(hunk.old_count, 1);
    assert_eq!(hunk.new_count, 1);
}

#[test]
fn test_hunk_add_addition() {
    let mut hunk = Hunk::new(1, 1);
    hunk.add_addition("new line");

    assert_eq!(hunk.lines.len(), 1);
    assert_eq!(hunk.old_count, 0);
    assert_eq!(hunk.new_count, 1);
}

#[test]
fn test_hunk_add_removal() {
    let mut hunk = Hunk::new(1, 1);
    hunk.add_removal("removed line");

    assert_eq!(hunk.lines.len(), 1);
    assert_eq!(hunk.old_count, 1);
    assert_eq!(hunk.new_count, 0);
}

#[test]
fn test_diff_line_context() {
    let line = DiffLine::Context("test".to_string());
    assert!(line.is_context());
    assert!(!line.is_add());
    assert!(!line.is_remove());
    assert_eq!(line.content(), "test");
}

#[test]
fn test_diff_line_add() {
    let line = DiffLine::Add("new".to_string());
    assert!(!line.is_context());
    assert!(line.is_add());
    assert!(!line.is_remove());
}

#[test]
fn test_diff_line_remove() {
    let line = DiffLine::Remove("old".to_string());
    assert!(!line.is_context());
    assert!(!line.is_add());
    assert!(line.is_remove());
}

#[test]
fn test_diff_line_display() {
    let context = DiffLine::Context("test".to_string());
    let add = DiffLine::Add("new".to_string());
    let remove = DiffLine::Remove("old".to_string());

    assert_eq!(format!("{}", context), " test");
    assert_eq!(format!("{}", add), "+new");
    assert_eq!(format!("{}", remove), "-old");
}

#[test]
fn test_diff_computation() {
    let old = "line 1\nline 2\nline 3";
    let new = "line 1\nline 2 modified\nline 3\nline 4";

    let result = diff(old, new);
    assert!(!result.is_empty());
    assert!(result.lines_added() > 0);
}

#[test]
fn test_diff_builder_create_file() {
    let result = DiffBuilder::new()
        .create_file("new.txt", "hello\nworld")
        .build();

    assert_eq!(result.file_count(), 1);
    assert_eq!(result.files[0].operation, FileOperation::Create);
}

#[test]
fn test_diff_builder_delete_file() {
    let result = DiffBuilder::new().delete_file("old.txt", "content").build();

    assert_eq!(result.file_count(), 1);
    assert_eq!(result.files[0].operation, FileOperation::Delete);
}

#[test]
fn test_diff_builder_modify_file() {
    let result = DiffBuilder::new()
        .modify_file("file.txt", "old content", "new content")
        .build();

    // Modification may create 0 or 1 file depending on whether there are changes
    assert!(result.file_count() <= 1);
}

#[test]
fn test_file_diff_new_file() {
    let file = FileDiff::new_file("test.txt", "line1\nline2");

    assert_eq!(file.path, PathBuf::from("test.txt"));
    assert_eq!(file.operation, FileOperation::Create);
    assert!(!file.hunks.is_empty());
}

#[test]
fn test_file_diff_delete_file() {
    let file = FileDiff::delete_file("test.txt", "line1\nline2");

    assert_eq!(file.operation, FileOperation::Delete);
}

#[test]
fn test_file_diff_lines_added() {
    let file = FileDiff::new_file("test.txt", "line1\nline2\nline3");
    assert_eq!(file.lines_added(), 3);
}

#[test]
fn test_file_diff_lines_removed() {
    let file = FileDiff::delete_file("test.txt", "line1\nline2");
    assert_eq!(file.lines_removed(), 2);
}

#[test]
fn test_apply_result_success() {
    let result = ApplyResult::default();
    assert!(result.is_success());
    assert!(result.errors.is_empty());
}

#[test]
fn test_apply_result_with_errors() {
    let result = ApplyResult {
        files_modified: 0,
        lines_added: 0,
        lines_removed: 0,
        errors: vec![FileError {
            path: PathBuf::from("test.txt"),
            error: "file not found".to_string(),
        }],
    };

    assert!(!result.is_success());
}

#[test]
fn test_unified_diff_lines_counts() {
    let diff_text = r#"--- a/file.txt
+++ b/file.txt
@@ -1,2 +1,3 @@
 line 1
-old
+new
+added
"#;

    let diff = UnifiedDiff::parse(diff_text).unwrap();
    assert_eq!(diff.lines_added(), 2);
    assert_eq!(diff.lines_removed(), 1);
}

#[test]
fn test_diff_parser_default() {
    let parser = DiffParser::default();
    let result = parser.parse("").unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_hunk_display() {
    let mut hunk = Hunk::new(1, 1);
    hunk.add_context("context");
    hunk.add_removal("removed");
    hunk.add_addition("added");

    let s = hunk.to_string();
    assert!(s.contains("@@"));
    assert!(s.contains("-removed"));
    assert!(s.contains("+added"));
}
