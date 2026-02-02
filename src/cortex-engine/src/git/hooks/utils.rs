//! Utility Functions.
//!
//! Helper functions for git hooks.

use std::path::Path;

/// Check if a path should be ignored for scanning.
pub fn should_ignore_path(path: &Path) -> bool {
    let path_str = path.to_string_lossy();

    // Ignore patterns
    let ignore_patterns = [
        ".git",
        "node_modules",
        "target",
        "dist",
        "build",
        ".next",
        "__pycache__",
        ".pyc",
        ".class",
        ".lock",
        "package-lock.json",
        "yarn.lock",
        "Cargo.lock",
        ".min.js",
        ".min.css",
        ".map",
        ".svg",
        ".png",
        ".jpg",
        ".jpeg",
        ".gif",
        ".ico",
        ".woff",
        ".woff2",
        ".ttf",
        ".eot",
    ];

    ignore_patterns.iter().any(|p| path_str.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_ignore_path() {
        assert!(should_ignore_path(Path::new(
            "node_modules/package/index.js"
        )));
        assert!(should_ignore_path(Path::new("target/debug/binary")));
        assert!(should_ignore_path(Path::new("image.png")));
        assert!(!should_ignore_path(Path::new("src/main.rs")));
    }
}
