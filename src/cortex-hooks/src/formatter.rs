//! Built-in formatters for common file types.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Formatter configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatterConfig {
    /// Formatter identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Command to run.
    pub command: Vec<String>,
    /// File extensions this formatter handles.
    pub extensions: Vec<String>,
    /// Environment variables.
    #[serde(default)]
    pub environment: HashMap<String, String>,
    /// Whether formatter is disabled.
    #[serde(default)]
    pub disabled: bool,
}

impl FormatterConfig {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            command: Vec::new(),
            extensions: Vec::new(),
            environment: HashMap::new(),
            disabled: false,
        }
    }

    pub fn command(mut self, cmd: Vec<&str>) -> Self {
        self.command = cmd.into_iter().map(String::from).collect();
        self
    }

    pub fn extensions(mut self, exts: Vec<&str>) -> Self {
        self.extensions = exts.into_iter().map(String::from).collect();
        self
    }

    pub fn matches_extension(&self, ext: &str) -> bool {
        self.extensions.iter().any(|e| e == ext)
    }

    pub fn matches_file(&self, path: &str) -> bool {
        if let Some(ext) = std::path::Path::new(path).extension() {
            self.matches_extension(&ext.to_string_lossy())
        } else {
            false
        }
    }
}

/// A formatter instance.
pub struct Formatter {
    pub config: FormatterConfig,
}

impl Formatter {
    pub fn new(config: FormatterConfig) -> Self {
        Self { config }
    }

    /// Build command for formatting a file.
    pub fn build_command(&self, file_path: &str) -> Vec<String> {
        self.config
            .command
            .iter()
            .map(|arg| arg.replace("{file}", file_path))
            .collect()
    }
}

lazy_static::lazy_static! {
    pub static ref BUILTIN_FORMATTERS: Vec<FormatterConfig> = vec![
        // Prettier - JS/TS/HTML/CSS/JSON/YAML/MD
        FormatterConfig::new("prettier", "Prettier")
            .command(vec!["prettier", "--write", "{file}"])
            .extensions(vec![
                "js", "jsx", "ts", "tsx", "mjs", "cjs",
                "json", "jsonc", "json5",
                "html", "htm", "vue", "svelte",
                "css", "scss", "less",
                "md", "mdx",
                "yaml", "yml",
                "graphql", "gql"
            ]),

        // Black - Python
        FormatterConfig::new("black", "Black")
            .command(vec!["black", "{file}"])
            .extensions(vec!["py", "pyi"]),

        // Ruff - Python (faster alternative)
        FormatterConfig::new("ruff", "Ruff")
            .command(vec!["ruff", "format", "{file}"])
            .extensions(vec!["py", "pyi"]),

        // Rustfmt - Rust
        FormatterConfig::new("rustfmt", "Rustfmt")
            .command(vec!["rustfmt", "{file}"])
            .extensions(vec!["rs"]),

        // gofmt - Go
        FormatterConfig::new("gofmt", "Gofmt")
            .command(vec!["gofmt", "-w", "{file}"])
            .extensions(vec!["go"]),

        // goimports - Go (with import sorting)
        FormatterConfig::new("goimports", "Goimports")
            .command(vec!["goimports", "-w", "{file}"])
            .extensions(vec!["go"]),

        // clang-format - C/C++
        FormatterConfig::new("clang-format", "Clang-Format")
            .command(vec!["clang-format", "-i", "{file}"])
            .extensions(vec!["c", "cpp", "cc", "cxx", "h", "hpp", "hxx"]),

        // shfmt - Shell
        FormatterConfig::new("shfmt", "Shfmt")
            .command(vec!["shfmt", "-w", "{file}"])
            .extensions(vec!["sh", "bash"]),

        // stylua - Lua
        FormatterConfig::new("stylua", "StyLua")
            .command(vec!["stylua", "{file}"])
            .extensions(vec!["lua"]),

        // mix format - Elixir
        FormatterConfig::new("mix-format", "Mix Format")
            .command(vec!["mix", "format", "{file}"])
            .extensions(vec!["ex", "exs"]),

        // zigfmt - Zig
        FormatterConfig::new("zigfmt", "Zig Fmt")
            .command(vec!["zig", "fmt", "{file}"])
            .extensions(vec!["zig"]),

        // terraform fmt - Terraform
        FormatterConfig::new("terraform-fmt", "Terraform Fmt")
            .command(vec!["terraform", "fmt", "{file}"])
            .extensions(vec!["tf", "tfvars"]),

        // SQL formatter
        FormatterConfig::new("sql-formatter", "SQL Formatter")
            .command(vec!["sql-formatter", "--fix", "{file}"])
            .extensions(vec!["sql"]),

        // xmllint - XML
        FormatterConfig::new("xmllint", "XMLLint")
            .command(vec!["xmllint", "--format", "--output", "{file}", "{file}"])
            .extensions(vec!["xml", "xsl", "xslt"]),

        // rubocop - Ruby
        FormatterConfig::new("rubocop", "RuboCop")
            .command(vec!["rubocop", "-a", "{file}"])
            .extensions(vec!["rb", "rake"]),

        // ktlint - Kotlin
        FormatterConfig::new("ktlint", "ktlint")
            .command(vec!["ktlint", "-F", "{file}"])
            .extensions(vec!["kt", "kts"]),

        // swift-format - Swift
        FormatterConfig::new("swift-format", "Swift Format")
            .command(vec!["swift-format", "--in-place", "{file}"])
            .extensions(vec!["swift"]),

        // dart format - Dart
        FormatterConfig::new("dart-format", "Dart Format")
            .command(vec!["dart", "format", "{file}"])
            .extensions(vec!["dart"]),
    ];
}

/// Find a formatter for a file.
pub fn find_formatter_for_file(path: &str) -> Option<&'static FormatterConfig> {
    BUILTIN_FORMATTERS.iter().find(|f| f.matches_file(path))
}

/// Find a formatter by ID.
pub fn find_formatter_by_id(id: &str) -> Option<&'static FormatterConfig> {
    BUILTIN_FORMATTERS.iter().find(|f| f.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_formatter() {
        let formatter = find_formatter_for_file("main.rs");
        assert!(formatter.is_some());
        assert_eq!(formatter.unwrap().id, "rustfmt");

        let formatter = find_formatter_for_file("app.tsx");
        assert!(formatter.is_some());
        assert_eq!(formatter.unwrap().id, "prettier");
    }

    #[test]
    fn test_formatter_command() {
        let config = FormatterConfig::new("test", "Test").command(vec!["fmt", "--write", "{file}"]);
        let formatter = Formatter::new(config);

        let cmd = formatter.build_command("/path/to/file.rs");
        assert_eq!(cmd[2], "/path/to/file.rs");
    }
}
