//! Language utilities.
//!
//! Provides utilities for working with programming languages
//! including syntax detection, formatting, and analysis.

use std::collections::HashMap;
use std::path::Path;

/// Programming language info.
#[derive(Debug, Clone)]
pub struct LanguageInfo {
    /// Language ID.
    pub id: &'static str,
    /// Display name.
    pub name: &'static str,
    /// File extensions.
    pub extensions: &'static [&'static str],
    /// Common filenames.
    pub filenames: &'static [&'static str],
    /// Line comment prefix.
    pub line_comment: Option<&'static str>,
    /// Block comment start.
    pub block_comment_start: Option<&'static str>,
    /// Block comment end.
    pub block_comment_end: Option<&'static str>,
    /// String delimiters.
    pub string_delimiters: &'static [char],
    /// Keywords.
    pub keywords: &'static [&'static str],
    /// Is compiled.
    pub compiled: bool,
    /// Has type system.
    pub typed: bool,
}

/// Get language info by ID.
pub fn get_language(id: &str) -> Option<LanguageInfo> {
    LANGUAGES.iter().find(|l| l.id == id).cloned()
}

/// Detect language from file path.
pub fn detect_from_path(path: &Path) -> Option<LanguageInfo> {
    // Check filename first
    if let Some(filename) = path.file_name().and_then(|f| f.to_str()) {
        for lang in LANGUAGES.iter() {
            if lang.filenames.contains(&filename) {
                return Some(lang.clone());
            }
        }
    }

    // Check extension
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        for lang in LANGUAGES.iter() {
            if lang.extensions.contains(&ext) {
                return Some(lang.clone());
            }
        }
    }

    None
}

/// Detect language from content.
pub fn detect_from_content(content: &str) -> Option<LanguageInfo> {
    // Check shebang
    if content.starts_with("#!") {
        let first_line = content.lines().next().unwrap_or("");

        if first_line.contains("python") {
            return get_language("python");
        } else if first_line.contains("node") || first_line.contains("nodejs") {
            return get_language("javascript");
        } else if first_line.contains("bash") || first_line.contains("sh") {
            return get_language("shell");
        } else if first_line.contains("ruby") {
            return get_language("ruby");
        } else if first_line.contains("perl") {
            return get_language("perl");
        }
    }

    // Check for language-specific patterns
    let patterns: Vec<(&str, Vec<&str>)> = vec![
        (
            "rust",
            vec!["fn main", "impl ", "pub struct", "use std::", "#[derive"],
        ),
        (
            "python",
            vec!["def ", "import ", "class ", "if __name__", "from "],
        ),
        (
            "javascript",
            vec!["const ", "function ", "=>", "require(", "export "],
        ),
        (
            "typescript",
            vec!["interface ", ": string", ": number", "type ", "as "],
        ),
        ("go", vec!["package ", "func ", "import (", "type struct"]),
        (
            "java",
            vec!["public class", "private ", "public static", "import java"],
        ),
        (
            "csharp",
            vec!["namespace ", "public class", "using System", "private void"],
        ),
        ("ruby", vec!["require ", "class ", "def ", "end\n", "attr_"]),
        (
            "php",
            vec!["<?php", "function ", "class ", "public function"],
        ),
        (
            "swift",
            vec!["import Foundation", "func ", "var ", "let ", "guard "],
        ),
        ("kotlin", vec!["fun ", "val ", "var ", "class ", "package "]),
    ];

    let mut scores: HashMap<&str, usize> = HashMap::new();

    for (lang, lang_patterns) in &patterns {
        for pattern in lang_patterns {
            if content.contains(*pattern) {
                *scores.entry(*lang).or_insert(0) += 1;
            }
        }
    }

    scores
        .into_iter()
        .max_by_key(|(_, score)| *score)
        .filter(|(_, score)| *score > 0)
        .and_then(|(lang, _)| get_language(lang))
}

/// Language definitions.
static LANGUAGES: &[LanguageInfo] = &[
    LanguageInfo {
        id: "rust",
        name: "Rust",
        extensions: &["rs"],
        filenames: &["Cargo.toml", "Cargo.lock"],
        line_comment: Some("//"),
        block_comment_start: Some("/*"),
        block_comment_end: Some("*/"),
        string_delimiters: &['"'],
        keywords: &[
            "fn", "let", "mut", "pub", "use", "mod", "struct", "enum", "impl", "trait", "where",
            "for", "if", "else", "match", "loop", "while", "return", "async", "await",
        ],
        compiled: true,
        typed: true,
    },
    LanguageInfo {
        id: "python",
        name: "Python",
        extensions: &["py", "pyw", "pyi"],
        filenames: &["requirements.txt", "setup.py", "pyproject.toml"],
        line_comment: Some("#"),
        block_comment_start: Some("\"\"\""),
        block_comment_end: Some("\"\"\""),
        string_delimiters: &['"', '\''],
        keywords: &[
            "def", "class", "if", "else", "elif", "for", "while", "import", "from", "return",
            "yield", "with", "as", "try", "except", "finally", "raise", "lambda", "and", "or",
            "not", "in", "is", "True", "False", "None", "async", "await",
        ],
        compiled: false,
        typed: false,
    },
    LanguageInfo {
        id: "javascript",
        name: "JavaScript",
        extensions: &["js", "mjs", "cjs", "jsx"],
        filenames: &["package.json", ".eslintrc.js", "webpack.config.js"],
        line_comment: Some("//"),
        block_comment_start: Some("/*"),
        block_comment_end: Some("*/"),
        string_delimiters: &['"', '\'', '`'],
        keywords: &[
            "function", "const", "let", "var", "if", "else", "for", "while", "return", "class",
            "extends", "new", "this", "async", "await", "import", "export", "default", "try",
            "catch", "finally", "throw",
        ],
        compiled: false,
        typed: false,
    },
    LanguageInfo {
        id: "typescript",
        name: "TypeScript",
        extensions: &["ts", "tsx", "mts", "cts"],
        filenames: &["tsconfig.json"],
        line_comment: Some("//"),
        block_comment_start: Some("/*"),
        block_comment_end: Some("*/"),
        string_delimiters: &['"', '\'', '`'],
        keywords: &[
            "function",
            "const",
            "let",
            "var",
            "if",
            "else",
            "for",
            "while",
            "return",
            "class",
            "extends",
            "new",
            "this",
            "async",
            "await",
            "import",
            "export",
            "default",
            "try",
            "catch",
            "finally",
            "throw",
            "interface",
            "type",
            "enum",
            "namespace",
            "abstract",
            "implements",
            "public",
            "private",
            "protected",
        ],
        compiled: true,
        typed: true,
    },
    LanguageInfo {
        id: "go",
        name: "Go",
        extensions: &["go"],
        filenames: &["go.mod", "go.sum"],
        line_comment: Some("//"),
        block_comment_start: Some("/*"),
        block_comment_end: Some("*/"),
        string_delimiters: &['"', '`'],
        keywords: &[
            "package",
            "import",
            "func",
            "var",
            "const",
            "type",
            "struct",
            "interface",
            "map",
            "chan",
            "go",
            "defer",
            "if",
            "else",
            "for",
            "range",
            "switch",
            "case",
            "default",
            "select",
            "return",
            "break",
            "continue",
            "fallthrough",
        ],
        compiled: true,
        typed: true,
    },
    LanguageInfo {
        id: "java",
        name: "Java",
        extensions: &["java"],
        filenames: &["pom.xml", "build.gradle"],
        line_comment: Some("//"),
        block_comment_start: Some("/*"),
        block_comment_end: Some("*/"),
        string_delimiters: &['"'],
        keywords: &[
            "public",
            "private",
            "protected",
            "class",
            "interface",
            "extends",
            "implements",
            "static",
            "final",
            "void",
            "int",
            "long",
            "double",
            "boolean",
            "if",
            "else",
            "for",
            "while",
            "do",
            "switch",
            "case",
            "break",
            "continue",
            "return",
            "new",
            "this",
            "super",
            "try",
            "catch",
            "finally",
            "throw",
            "throws",
            "import",
            "package",
        ],
        compiled: true,
        typed: true,
    },
    LanguageInfo {
        id: "csharp",
        name: "C#",
        extensions: &["cs"],
        filenames: &[".csproj"],
        line_comment: Some("//"),
        block_comment_start: Some("/*"),
        block_comment_end: Some("*/"),
        string_delimiters: &['"'],
        keywords: &[
            "public",
            "private",
            "protected",
            "class",
            "interface",
            "struct",
            "enum",
            "namespace",
            "using",
            "static",
            "void",
            "int",
            "string",
            "bool",
            "if",
            "else",
            "for",
            "foreach",
            "while",
            "do",
            "switch",
            "case",
            "break",
            "continue",
            "return",
            "new",
            "this",
            "base",
            "try",
            "catch",
            "finally",
            "throw",
            "async",
            "await",
            "var",
        ],
        compiled: true,
        typed: true,
    },
    LanguageInfo {
        id: "cpp",
        name: "C++",
        extensions: &["cpp", "cc", "cxx", "hpp", "hxx", "h"],
        filenames: &["CMakeLists.txt", "Makefile"],
        line_comment: Some("//"),
        block_comment_start: Some("/*"),
        block_comment_end: Some("*/"),
        string_delimiters: &['"'],
        keywords: &[
            "class",
            "struct",
            "enum",
            "namespace",
            "using",
            "template",
            "typename",
            "public",
            "private",
            "protected",
            "virtual",
            "override",
            "static",
            "const",
            "void",
            "int",
            "long",
            "double",
            "bool",
            "if",
            "else",
            "for",
            "while",
            "do",
            "switch",
            "case",
            "break",
            "continue",
            "return",
            "new",
            "delete",
            "this",
            "try",
            "catch",
            "throw",
            "auto",
            "nullptr",
        ],
        compiled: true,
        typed: true,
    },
    LanguageInfo {
        id: "c",
        name: "C",
        extensions: &["c", "h"],
        filenames: &["Makefile"],
        line_comment: Some("//"),
        block_comment_start: Some("/*"),
        block_comment_end: Some("*/"),
        string_delimiters: &['"'],
        keywords: &[
            "auto", "break", "case", "char", "const", "continue", "default", "do", "double",
            "else", "enum", "extern", "float", "for", "goto", "if", "int", "long", "register",
            "return", "short", "signed", "sizeof", "static", "struct", "switch", "typedef",
            "union", "unsigned", "void", "volatile", "while",
        ],
        compiled: true,
        typed: true,
    },
    LanguageInfo {
        id: "ruby",
        name: "Ruby",
        extensions: &["rb", "rake"],
        filenames: &["Gemfile", "Rakefile", ".ruby-version"],
        line_comment: Some("#"),
        block_comment_start: Some("=begin"),
        block_comment_end: Some("=end"),
        string_delimiters: &['"', '\''],
        keywords: &[
            "def",
            "end",
            "class",
            "module",
            "if",
            "else",
            "elsif",
            "unless",
            "case",
            "when",
            "while",
            "until",
            "for",
            "do",
            "begin",
            "rescue",
            "ensure",
            "raise",
            "return",
            "yield",
            "self",
            "super",
            "nil",
            "true",
            "false",
            "and",
            "or",
            "not",
            "in",
            "require",
            "include",
            "extend",
            "attr_accessor",
            "attr_reader",
            "attr_writer",
        ],
        compiled: false,
        typed: false,
    },
    LanguageInfo {
        id: "php",
        name: "PHP",
        extensions: &["php", "phtml"],
        filenames: &["composer.json"],
        line_comment: Some("//"),
        block_comment_start: Some("/*"),
        block_comment_end: Some("*/"),
        string_delimiters: &['"', '\''],
        keywords: &[
            "function",
            "class",
            "interface",
            "trait",
            "extends",
            "implements",
            "public",
            "private",
            "protected",
            "static",
            "final",
            "abstract",
            "if",
            "else",
            "elseif",
            "for",
            "foreach",
            "while",
            "do",
            "switch",
            "case",
            "break",
            "continue",
            "return",
            "new",
            "echo",
            "print",
            "require",
            "include",
            "use",
            "namespace",
            "try",
            "catch",
            "finally",
            "throw",
        ],
        compiled: false,
        typed: false,
    },
    LanguageInfo {
        id: "swift",
        name: "Swift",
        extensions: &["swift"],
        filenames: &["Package.swift"],
        line_comment: Some("//"),
        block_comment_start: Some("/*"),
        block_comment_end: Some("*/"),
        string_delimiters: &['"'],
        keywords: &[
            "func",
            "var",
            "let",
            "class",
            "struct",
            "enum",
            "protocol",
            "extension",
            "if",
            "else",
            "guard",
            "for",
            "while",
            "repeat",
            "switch",
            "case",
            "break",
            "continue",
            "return",
            "throw",
            "throws",
            "try",
            "catch",
            "defer",
            "import",
            "public",
            "private",
            "internal",
            "fileprivate",
            "open",
            "static",
            "final",
            "override",
            "init",
            "self",
            "Self",
            "super",
            "nil",
            "true",
            "false",
        ],
        compiled: true,
        typed: true,
    },
    LanguageInfo {
        id: "kotlin",
        name: "Kotlin",
        extensions: &["kt", "kts"],
        filenames: &["build.gradle.kts"],
        line_comment: Some("//"),
        block_comment_start: Some("/*"),
        block_comment_end: Some("*/"),
        string_delimiters: &['"'],
        keywords: &[
            "fun",
            "val",
            "var",
            "class",
            "object",
            "interface",
            "enum",
            "sealed",
            "data",
            "if",
            "else",
            "when",
            "for",
            "while",
            "do",
            "return",
            "break",
            "continue",
            "throw",
            "try",
            "catch",
            "finally",
            "import",
            "package",
            "public",
            "private",
            "protected",
            "internal",
            "open",
            "final",
            "override",
            "abstract",
            "companion",
            "this",
            "super",
            "null",
            "true",
            "false",
            "is",
            "as",
            "in",
        ],
        compiled: true,
        typed: true,
    },
    LanguageInfo {
        id: "shell",
        name: "Shell",
        extensions: &["sh", "bash", "zsh"],
        filenames: &[".bashrc", ".zshrc", ".profile"],
        line_comment: Some("#"),
        block_comment_start: None,
        block_comment_end: None,
        string_delimiters: &['"', '\''],
        keywords: &[
            "if", "then", "else", "elif", "fi", "case", "esac", "for", "while", "until", "do",
            "done", "function", "return", "exit", "break", "continue", "export", "local",
            "readonly", "declare", "source", "alias", "unset", "shift", "trap",
        ],
        compiled: false,
        typed: false,
    },
    LanguageInfo {
        id: "sql",
        name: "SQL",
        extensions: &["sql"],
        filenames: &[],
        line_comment: Some("--"),
        block_comment_start: Some("/*"),
        block_comment_end: Some("*/"),
        string_delimiters: &['\''],
        keywords: &[
            "SELECT", "FROM", "WHERE", "AND", "OR", "NOT", "IN", "LIKE", "IS", "NULL", "ORDER",
            "BY", "ASC", "DESC", "LIMIT", "OFFSET", "INSERT", "INTO", "VALUES", "UPDATE", "SET",
            "DELETE", "CREATE", "TABLE", "ALTER", "DROP", "INDEX", "VIEW", "JOIN", "LEFT", "RIGHT",
            "INNER", "OUTER", "ON", "GROUP", "HAVING", "UNION", "DISTINCT", "AS", "CASE", "WHEN",
            "THEN", "ELSE", "END",
        ],
        compiled: false,
        typed: true,
    },
    LanguageInfo {
        id: "html",
        name: "HTML",
        extensions: &["html", "htm"],
        filenames: &["index.html"],
        line_comment: None,
        block_comment_start: Some("<!--"),
        block_comment_end: Some("-->"),
        string_delimiters: &['"', '\''],
        keywords: &[],
        compiled: false,
        typed: false,
    },
    LanguageInfo {
        id: "css",
        name: "CSS",
        extensions: &["css", "scss", "sass", "less"],
        filenames: &[],
        line_comment: None,
        block_comment_start: Some("/*"),
        block_comment_end: Some("*/"),
        string_delimiters: &['"', '\''],
        keywords: &[],
        compiled: false,
        typed: false,
    },
    LanguageInfo {
        id: "json",
        name: "JSON",
        extensions: &["json", "jsonc"],
        filenames: &["package.json", "tsconfig.json"],
        line_comment: None,
        block_comment_start: None,
        block_comment_end: None,
        string_delimiters: &['"'],
        keywords: &[],
        compiled: false,
        typed: false,
    },
    LanguageInfo {
        id: "yaml",
        name: "YAML",
        extensions: &["yaml", "yml"],
        filenames: &[".travis.yml", "docker-compose.yml"],
        line_comment: Some("#"),
        block_comment_start: None,
        block_comment_end: None,
        string_delimiters: &['"', '\''],
        keywords: &[],
        compiled: false,
        typed: false,
    },
    LanguageInfo {
        id: "toml",
        name: "TOML",
        extensions: &["toml"],
        filenames: &["Cargo.toml", "pyproject.toml"],
        line_comment: Some("#"),
        block_comment_start: None,
        block_comment_end: None,
        string_delimiters: &['"', '\''],
        keywords: &[],
        compiled: false,
        typed: false,
    },
    LanguageInfo {
        id: "markdown",
        name: "Markdown",
        extensions: &["md", "markdown"],
        filenames: &["README.md", "CHANGELOG.md"],
        line_comment: None,
        block_comment_start: None,
        block_comment_end: None,
        string_delimiters: &[],
        keywords: &[],
        compiled: false,
        typed: false,
    },
    LanguageInfo {
        id: "perl",
        name: "Perl",
        extensions: &["pl", "pm"],
        filenames: &[],
        line_comment: Some("#"),
        block_comment_start: Some("=pod"),
        block_comment_end: Some("=cut"),
        string_delimiters: &['"', '\''],
        keywords: &[
            "sub", "my", "our", "local", "if", "else", "elsif", "unless", "while", "until", "for",
            "foreach", "do", "use", "require", "package", "return", "last", "next", "redo", "goto",
            "die", "warn", "print", "say", "open", "close", "read", "write",
        ],
        compiled: false,
        typed: false,
    },
];

/// Get all languages.
pub fn all_languages() -> &'static [LanguageInfo] {
    LANGUAGES
}

/// Get language by extension.
pub fn get_by_extension(ext: &str) -> Option<LanguageInfo> {
    LANGUAGES
        .iter()
        .find(|l| l.extensions.contains(&ext))
        .cloned()
}

/// Get language by filename.
pub fn get_by_filename(name: &str) -> Option<LanguageInfo> {
    LANGUAGES
        .iter()
        .find(|l| l.filenames.contains(&name))
        .cloned()
}

/// Check if language is compiled.
pub fn is_compiled(id: &str) -> bool {
    get_language(id).map(|l| l.compiled).unwrap_or(false)
}

/// Check if language has types.
pub fn is_typed(id: &str) -> bool {
    get_language(id).map(|l| l.typed).unwrap_or(false)
}

/// Get comment for language.
pub fn get_comment(id: &str) -> Option<&'static str> {
    get_language(id).and_then(|l| l.line_comment)
}

/// Format code comment.
pub fn format_comment(id: &str, text: &str) -> String {
    if let Some(lang) = get_language(id)
        && let Some(prefix) = lang.line_comment
    {
        return text
            .lines()
            .map(|line| format!("{prefix} {line}"))
            .collect::<Vec<_>>()
            .join("\n");
    }
    text.to_string()
}

/// Extract comments from code.
pub fn extract_comments(id: &str, code: &str) -> Vec<String> {
    let mut comments = Vec::new();

    if let Some(lang) = get_language(id) {
        let lines: Vec<&str> = code.lines().collect();
        let mut in_block = false;

        for line in lines {
            let trimmed = line.trim();

            // Check block comments
            if let (Some(start), Some(end)) = (lang.block_comment_start, lang.block_comment_end) {
                if trimmed.contains(start) {
                    in_block = true;
                }
                if in_block {
                    comments.push(trimmed.to_string());
                    if trimmed.contains(end) {
                        in_block = false;
                    }
                    continue;
                }
            }

            // Check line comments
            if let Some(prefix) = lang.line_comment
                && trimmed.starts_with(prefix)
            {
                comments.push(trimmed[prefix.len()..].trim().to_string());
            }
        }
    }

    comments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_language() {
        let rust = get_language("rust");
        assert!(rust.is_some());
        assert_eq!(rust.unwrap().name, "Rust");

        let unknown = get_language("unknown");
        assert!(unknown.is_none());
    }

    #[test]
    fn test_detect_from_path() {
        let rust = detect_from_path(Path::new("main.rs"));
        assert!(rust.is_some());
        assert_eq!(rust.unwrap().id, "rust");

        let python = detect_from_path(Path::new("script.py"));
        assert!(python.is_some());
        assert_eq!(python.unwrap().id, "python");
    }

    #[test]
    fn test_detect_from_content() {
        let rust = detect_from_content("fn main() { println!(\"Hello\"); }");
        assert!(rust.is_some());
        assert_eq!(rust.unwrap().id, "rust");

        let python = detect_from_content(
            "def hello():\n    print('Hello')\n if __name__ == 'main':\n    hello()",
        );
        assert!(python.is_some());
        assert_eq!(python.unwrap().id, "python");
    }

    #[test]
    fn test_get_by_extension() {
        assert!(get_by_extension("rs").is_some());
        assert!(get_by_extension("py").is_some());
        assert!(get_by_extension("js").is_some());
    }

    #[test]
    fn test_format_comment() {
        let rust_comment = format_comment("rust", "Hello\nWorld");
        assert!(rust_comment.contains("// Hello"));
        assert!(rust_comment.contains("// World"));

        let python_comment = format_comment("python", "Hello");
        assert!(python_comment.contains("# Hello"));
    }

    #[test]
    fn test_extract_comments() {
        let code = "// This is a comment\nfn main() {}\n// Another comment";
        let comments = extract_comments("rust", code);
        assert_eq!(comments.len(), 2);
    }

    #[test]
    fn test_is_compiled() {
        assert!(is_compiled("rust"));
        assert!(is_compiled("go"));
        assert!(!is_compiled("python"));
        assert!(!is_compiled("javascript"));
    }

    #[test]
    fn test_is_typed() {
        assert!(is_typed("rust"));
        assert!(is_typed("typescript"));
        assert!(!is_typed("python"));
        assert!(!is_typed("javascript"));
    }

    #[test]
    fn test_all_languages() {
        let langs = all_languages();
        assert!(langs.len() > 10);
    }
}
