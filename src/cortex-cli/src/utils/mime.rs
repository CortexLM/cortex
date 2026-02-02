//! MIME type detection utilities.
//!
//! Provides file extension to MIME type mapping for file attachments.

use std::path::Path;

/// Get the MIME type for a file based on its extension.
///
/// Returns a reasonable MIME type for common file extensions used in development.
///
/// # Arguments
/// * `path` - The file path to determine MIME type for
///
/// # Returns
/// The MIME type string (defaults to "application/octet-stream" for unknown types).
pub fn mime_type_from_path(path: &Path) -> &'static str {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    mime_type_from_extension(&extension)
}

/// Get the MIME type for a file extension.
///
/// # Arguments
/// * `extension` - The file extension (without the dot)
///
/// # Returns
/// The MIME type string.
pub fn mime_type_from_extension(extension: &str) -> &'static str {
    match extension.to_lowercase().as_str() {
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
        "jsx" => "text/javascript",
        "json" => "application/json",
        "jsonc" => "application/json",
        "json5" => "application/json",
        "rs" => "text/x-rust",
        "py" => "text/x-python",
        "pyi" => "text/x-python",
        "rb" => "text/x-ruby",
        "go" => "text/x-go",
        "java" => "text/x-java",
        "kt" | "kts" => "text/x-kotlin",
        "scala" => "text/x-scala",
        "c" | "h" => "text/x-c",
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" => "text/x-c++",
        "cs" => "text/x-csharp",
        "swift" => "text/x-swift",
        "m" | "mm" => "text/x-objective-c",
        "php" => "text/x-php",
        "pl" | "pm" => "text/x-perl",
        "r" => "text/x-r",
        "lua" => "text/x-lua",
        "hs" | "lhs" => "text/x-haskell",
        "erl" | "hrl" => "text/x-erlang",
        "ex" | "exs" => "text/x-elixir",
        "clj" | "cljs" | "cljc" => "text/x-clojure",
        "sql" => "text/x-sql",
        "sh" | "bash" | "zsh" => "text/x-shellscript",
        "ps1" | "psm1" | "psd1" => "text/x-powershell",
        "bat" | "cmd" => "text/x-batch",

        // Config files
        "yaml" | "yml" => "text/yaml",
        "toml" => "text/toml",
        "ini" => "text/plain",
        "cfg" | "conf" => "text/plain",
        "env" => "text/plain",
        "properties" => "text/plain",
        "editorconfig" => "text/plain",
        "gitignore" | "gitattributes" => "text/plain",
        "dockerignore" => "text/plain",
        "prettierrc" => "application/json",
        "eslintrc" => "application/json",

        // Markup/templating
        "vue" => "text/html",
        "svelte" => "text/html",
        "astro" => "text/html",
        "ejs" => "text/html",
        "hbs" | "handlebars" => "text/html",
        "pug" | "jade" => "text/plain",
        "slim" => "text/plain",
        "erb" => "text/html",
        "jinja" | "jinja2" | "j2" => "text/plain",
        "liquid" => "text/plain",

        // Documentation
        "rst" => "text/x-rst",
        "adoc" | "asciidoc" => "text/x-asciidoc",
        "tex" | "latex" => "text/x-latex",
        "org" => "text/x-org",

        // Data formats
        "graphql" | "gql" => "application/graphql",
        "proto" => "text/x-protobuf",
        "thrift" => "text/x-thrift",
        "avro" | "avsc" => "application/json",

        // Images
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "tiff" | "tif" => "image/tiff",

        // Documents
        "pdf" => "application/pdf",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "odp" => "application/vnd.oasis.opendocument.presentation",

        // Archives
        "zip" => "application/zip",
        "tar" => "application/x-tar",
        "gz" | "gzip" => "application/gzip",
        "bz2" => "application/x-bzip2",
        "xz" => "application/x-xz",
        "7z" => "application/x-7z-compressed",
        "rar" => "application/vnd.rar",

        // Executables and binaries
        "exe" => "application/x-msdownload",
        "dll" => "application/x-msdownload",
        "so" => "application/x-sharedlib",
        "dylib" => "application/x-sharedlib",
        "wasm" => "application/wasm",

        // Fonts
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "eot" => "application/vnd.ms-fontobject",

        // Audio/Video (common types)
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "mov" => "video/quicktime",

        // Certificates and keys
        "pem" => "application/x-pem-file",
        "crt" | "cer" => "application/x-x509-ca-cert",
        "key" => "application/x-pem-file",
        "p12" | "pfx" => "application/x-pkcs12",

        // Default
        _ => "application/octet-stream",
    }
}

/// Check if a MIME type indicates a text-based file.
///
/// # Arguments
/// * `mime_type` - The MIME type to check
///
/// # Returns
/// `true` if the MIME type indicates text content.
pub fn is_text_mime_type(mime_type: &str) -> bool {
    mime_type.starts_with("text/")
        || mime_type == "application/json"
        || mime_type == "application/xml"
        || mime_type == "application/graphql"
        || mime_type.ends_with("+json")
        || mime_type.ends_with("+xml")
}

/// Check if a MIME type indicates an image file.
///
/// # Arguments
/// * `mime_type` - The MIME type to check
///
/// # Returns
/// `true` if the MIME type indicates an image.
pub fn is_image_mime_type(mime_type: &str) -> bool {
    mime_type.starts_with("image/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_common_extensions() {
        assert_eq!(
            mime_type_from_path(&PathBuf::from("test.rs")),
            "text/x-rust"
        );
        assert_eq!(
            mime_type_from_path(&PathBuf::from("test.py")),
            "text/x-python"
        );
        assert_eq!(
            mime_type_from_path(&PathBuf::from("test.json")),
            "application/json"
        );
        assert_eq!(mime_type_from_path(&PathBuf::from("test.png")), "image/png");
    }

    #[test]
    fn test_unknown_extension() {
        assert_eq!(
            mime_type_from_path(&PathBuf::from("test.xyz123")),
            "application/octet-stream"
        );
    }

    #[test]
    fn test_is_text_mime_type() {
        assert!(is_text_mime_type("text/plain"));
        assert!(is_text_mime_type("text/x-rust"));
        assert!(is_text_mime_type("application/json"));
        assert!(!is_text_mime_type("image/png"));
        assert!(!is_text_mime_type("application/octet-stream"));
    }

    #[test]
    fn test_is_image_mime_type() {
        assert!(is_image_mime_type("image/png"));
        assert!(is_image_mime_type("image/jpeg"));
        assert!(!is_image_mime_type("text/plain"));
    }
}
