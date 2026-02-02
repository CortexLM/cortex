//! MIME type detection utilities.

use std::path::Path;

/// Determine MIME type from file extension.
pub fn mime_type_from_extension(path: &Path) -> String {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match extension.as_str() {
        // Text files
        "txt" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "csv" => "text/csv",
        "xml" => "text/xml",

        // Code files
        "js" | "mjs" | "cjs" => "text/javascript",
        "ts" | "tsx" => "text/typescript",
        "json" => "application/json",
        "rs" => "text/x-rust",
        "py" => "text/x-python",
        "rb" => "text/x-ruby",
        "go" => "text/x-go",
        "java" => "text/x-java",
        "c" | "h" => "text/x-c",
        "cpp" | "cc" | "cxx" | "hpp" => "text/x-c++",
        "sh" | "bash" | "zsh" => "text/x-shellscript",
        "ps1" => "text/x-powershell",
        "yaml" | "yml" => "text/yaml",
        "toml" => "text/toml",

        // Images
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",

        // Documents
        "pdf" => "application/pdf",
        "doc" | "docx" => "application/msword",
        "xls" | "xlsx" => "application/vnd.ms-excel",
        "ppt" | "pptx" => "application/vnd.ms-powerpoint",

        // Archives
        "zip" => "application/zip",
        "tar" => "application/x-tar",
        "gz" | "gzip" => "application/gzip",
        "7z" => "application/x-7z-compressed",

        _ => "application/octet-stream",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_mime_type_detection() {
        assert_eq!(
            mime_type_from_extension(&PathBuf::from("test.rs")),
            "text/x-rust"
        );
        assert_eq!(
            mime_type_from_extension(&PathBuf::from("test.json")),
            "application/json"
        );
        assert_eq!(
            mime_type_from_extension(&PathBuf::from("test.png")),
            "image/png"
        );
        assert_eq!(
            mime_type_from_extension(&PathBuf::from("test.unknown")),
            "application/octet-stream"
        );
    }
}
