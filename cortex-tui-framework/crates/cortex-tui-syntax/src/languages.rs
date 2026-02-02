//! Language registry for syntax highlighting.
//!
//! Provides language detection and grammar management for tree-sitter.

#![allow(clippy::non_std_lazy_statics)] // Using once_cell for broader compatibility

use ahash::AHashMap;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use std::path::Path;
use tree_sitter::Language;

/// File extension to language name mapping.
static EXTENSION_MAP: Lazy<AHashMap<&'static str, &'static str>> = Lazy::new(|| {
    let mut m = AHashMap::new();
    // Rust
    m.insert("rs", "rust");

    // JavaScript/TypeScript
    m.insert("js", "javascript");
    m.insert("mjs", "javascript");
    m.insert("cjs", "javascript");
    m.insert("jsx", "javascript");
    m.insert("ts", "typescript");
    m.insert("mts", "typescript");
    m.insert("cts", "typescript");
    m.insert("tsx", "tsx");

    // Web
    m.insert("html", "html");
    m.insert("htm", "html");
    m.insert("css", "css");
    m.insert("scss", "scss");
    m.insert("sass", "scss");
    m.insert("less", "css");
    m.insert("json", "json");
    m.insert("jsonc", "json");

    // Python
    m.insert("py", "python");
    m.insert("pyi", "python");
    m.insert("pyw", "python");

    // Go
    m.insert("go", "go");
    m.insert("mod", "gomod");

    // C/C++
    m.insert("c", "c");
    m.insert("h", "c");
    m.insert("cpp", "cpp");
    m.insert("cc", "cpp");
    m.insert("cxx", "cpp");
    m.insert("hpp", "cpp");
    m.insert("hxx", "cpp");

    // Java/Kotlin
    m.insert("java", "java");
    m.insert("kt", "kotlin");
    m.insert("kts", "kotlin");

    // Ruby
    m.insert("rb", "ruby");
    m.insert("rake", "ruby");
    m.insert("gemspec", "ruby");

    // Shell
    m.insert("sh", "bash");
    m.insert("bash", "bash");
    m.insert("zsh", "bash");
    m.insert("fish", "fish");

    // Config
    m.insert("yaml", "yaml");
    m.insert("yml", "yaml");
    m.insert("toml", "toml");
    m.insert("ini", "ini");
    m.insert("xml", "xml");

    // Markdown
    m.insert("md", "markdown");
    m.insert("markdown", "markdown");
    m.insert("mdx", "markdown");

    // Other
    m.insert("lua", "lua");
    m.insert("zig", "zig");
    m.insert("php", "php");
    m.insert("sql", "sql");
    m.insert("swift", "swift");
    m.insert("r", "r");
    m.insert("R", "r");
    m.insert("ex", "elixir");
    m.insert("exs", "elixir");
    m.insert("erl", "erlang");
    m.insert("hrl", "erlang");
    m.insert("hs", "haskell");
    m.insert("ml", "ocaml");
    m.insert("mli", "ocaml");
    m.insert("clj", "clojure");
    m.insert("cljs", "clojure");
    m.insert("scala", "scala");
    m.insert("vim", "vim");
    m.insert("dockerfile", "dockerfile");
    m.insert("proto", "protobuf");
    m.insert("graphql", "graphql");
    m.insert("gql", "graphql");
    m.insert("svelte", "svelte");
    m.insert("vue", "vue");

    m
});

/// Filename to language name mapping for special files.
static FILENAME_MAP: Lazy<AHashMap<&'static str, &'static str>> = Lazy::new(|| {
    let mut m = AHashMap::new();
    m.insert("Dockerfile", "dockerfile");
    m.insert("Makefile", "make");
    m.insert("makefile", "make");
    m.insert("GNUmakefile", "make");
    m.insert("CMakeLists.txt", "cmake");
    m.insert("Cargo.toml", "toml");
    m.insert("Cargo.lock", "toml");
    m.insert("package.json", "json");
    m.insert("tsconfig.json", "json");
    m.insert(".gitignore", "gitignore");
    m.insert(".gitattributes", "gitattributes");
    m.insert(".editorconfig", "editorconfig");
    m.insert("Gemfile", "ruby");
    m.insert("Rakefile", "ruby");
    m.insert("Jenkinsfile", "groovy");
    m.insert(".bashrc", "bash");
    m.insert(".bash_profile", "bash");
    m.insert(".zshrc", "bash");
    m.insert(".profile", "bash");
    m
});

/// Information about a registered language.
#[derive(Debug, Clone)]
pub struct LanguageInfo {
    /// The canonical language name.
    pub name: String,
    /// Display name for the language.
    pub display_name: String,
    /// Common file extensions.
    pub extensions: Vec<String>,
    /// Optional tree-sitter language (loaded lazily).
    language: Option<Language>,
    /// Highlight query source.
    pub highlight_query: Option<String>,
    /// Injection query source.
    pub injection_query: Option<String>,
}

impl LanguageInfo {
    /// Creates a new language info without a loaded grammar.
    pub fn new(name: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            display_name: display_name.into(),
            extensions: Vec::new(),
            language: None,
            highlight_query: None,
            injection_query: None,
        }
    }

    /// Creates a language info with a loaded grammar.
    pub fn with_language(
        name: impl Into<String>,
        display_name: impl Into<String>,
        language: Language,
    ) -> Self {
        Self {
            name: name.into(),
            display_name: display_name.into(),
            extensions: Vec::new(),
            language: Some(language),
            highlight_query: None,
            injection_query: None,
        }
    }

    /// Adds a file extension.
    pub fn extension(mut self, ext: impl Into<String>) -> Self {
        self.extensions.push(ext.into());
        self
    }

    /// Adds multiple file extensions.
    pub fn extensions(mut self, exts: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.extensions.extend(exts.into_iter().map(Into::into));
        self
    }

    /// Sets the highlight query.
    pub fn highlight_query(mut self, query: impl Into<String>) -> Self {
        self.highlight_query = Some(query.into());
        self
    }

    /// Sets the injection query.
    pub fn injection_query(mut self, query: impl Into<String>) -> Self {
        self.injection_query = Some(query.into());
        self
    }

    /// Sets the tree-sitter language.
    pub fn set_language(&mut self, language: Language) {
        self.language = Some(language);
    }

    /// Returns the tree-sitter language if loaded.
    pub fn language(&self) -> Option<&Language> {
        self.language.as_ref()
    }

    /// Returns true if the grammar is loaded.
    pub fn is_loaded(&self) -> bool {
        self.language.is_some()
    }
}

/// Registry for language grammars and configuration.
#[derive(Debug, Default)]
pub struct LanguageRegistry {
    /// Registered languages by name.
    languages: AHashMap<String, LanguageInfo>,
    /// Custom extension overrides.
    extension_overrides: AHashMap<String, String>,
}

impl LanguageRegistry {
    /// Creates a new empty language registry.
    pub fn new() -> Self {
        Self {
            languages: AHashMap::new(),
            extension_overrides: AHashMap::new(),
        }
    }

    /// Registers a language.
    pub fn register(&mut self, info: LanguageInfo) {
        let name = info.name.clone();
        for ext in &info.extensions {
            self.extension_overrides.insert(ext.clone(), name.clone());
        }
        self.languages.insert(name, info);
    }

    /// Gets language info by name.
    pub fn get(&self, name: &str) -> Option<&LanguageInfo> {
        self.languages.get(name)
    }

    /// Gets mutable language info by name.
    pub fn get_mut(&mut self, name: &str) -> Option<&mut LanguageInfo> {
        self.languages.get_mut(name)
    }

    /// Gets language info by file extension.
    pub fn get_by_extension(&self, ext: &str) -> Option<&LanguageInfo> {
        // Check custom overrides first
        if let Some(name) = self.extension_overrides.get(ext) {
            return self.languages.get(name);
        }
        // Fall back to built-in map
        if let Some(&name) = EXTENSION_MAP.get(ext) {
            return self.languages.get(name);
        }
        None
    }

    /// Returns true if a language is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.languages.contains_key(name)
    }

    /// Returns all registered language names.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.languages.keys().map(|s| s.as_str())
    }

    /// Returns all registered languages.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &LanguageInfo)> {
        self.languages.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Returns the number of registered languages.
    pub fn len(&self) -> usize {
        self.languages.len()
    }

    /// Returns true if no languages are registered.
    pub fn is_empty(&self) -> bool {
        self.languages.is_empty()
    }

    /// Adds a custom extension mapping.
    pub fn add_extension(&mut self, ext: impl Into<String>, language: impl Into<String>) {
        self.extension_overrides.insert(ext.into(), language.into());
    }
}

/// Global language registry singleton.
static GLOBAL_REGISTRY: Lazy<RwLock<LanguageRegistry>> =
    Lazy::new(|| RwLock::new(LanguageRegistry::new()));

/// Gets a reference to the global language registry.
pub fn global_registry() -> &'static RwLock<LanguageRegistry> {
    &GLOBAL_REGISTRY
}

/// Registers a language in the global registry.
pub fn register_language(info: LanguageInfo) {
    GLOBAL_REGISTRY.write().register(info);
}

/// Gets language name from a file extension.
///
/// Uses the built-in extension map.
pub fn language_from_extension(ext: &str) -> Option<&'static str> {
    // Normalize extension (remove leading dot if present)
    let ext = ext.strip_prefix('.').unwrap_or(ext);
    EXTENSION_MAP.get(ext).copied()
}

/// Gets language name from a file path.
///
/// Checks both filename and extension.
pub fn language_from_path(path: impl AsRef<Path>) -> Option<&'static str> {
    let path = path.as_ref();

    // Check filename first
    if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
        if let Some(&lang) = FILENAME_MAP.get(filename) {
            return Some(lang);
        }
    }

    // Check extension
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        return language_from_extension(ext);
    }

    None
}

/// Checks if a language is supported (has built-in extension mapping).
pub fn is_language_supported(name: &str) -> bool {
    EXTENSION_MAP.values().any(|&v| v == name)
}

/// Returns all supported language names.
pub fn supported_languages() -> impl Iterator<Item = &'static str> {
    // Deduplicate values
    let mut seen = std::collections::HashSet::new();
    EXTENSION_MAP
        .values()
        .copied()
        .filter(move |&lang| seen.insert(lang))
}

/// Returns all file extensions for a language.
pub fn extensions_for_language(lang: &str) -> Vec<&'static str> {
    EXTENSION_MAP
        .iter()
        .filter(|(_, &v)| v == lang)
        .map(|(&k, _)| k)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language_from_extension() {
        assert_eq!(language_from_extension("rs"), Some("rust"));
        assert_eq!(language_from_extension(".rs"), Some("rust"));
        assert_eq!(language_from_extension("js"), Some("javascript"));
        assert_eq!(language_from_extension("ts"), Some("typescript"));
        assert_eq!(language_from_extension("py"), Some("python"));
        assert_eq!(language_from_extension("unknown"), None);
    }

    #[test]
    fn test_language_from_path() {
        assert_eq!(language_from_path("main.rs"), Some("rust"));
        assert_eq!(language_from_path("/path/to/file.py"), Some("python"));
        assert_eq!(language_from_path("Dockerfile"), Some("dockerfile"));
        assert_eq!(language_from_path("Makefile"), Some("make"));
        assert_eq!(language_from_path("Cargo.toml"), Some("toml"));
    }

    #[test]
    fn test_language_registry() {
        let mut registry = LanguageRegistry::new();

        let info = LanguageInfo::new("test", "Test Language")
            .extension("tst")
            .extension("test");
        registry.register(info);

        assert!(registry.contains("test"));
        assert_eq!(
            registry.get_by_extension("tst").map(|l| &l.name),
            Some(&"test".to_string())
        );
    }

    #[test]
    fn test_supported_languages() {
        let langs: Vec<_> = supported_languages().collect();
        assert!(langs.contains(&"rust"));
        assert!(langs.contains(&"python"));
        assert!(langs.contains(&"javascript"));
    }

    #[test]
    fn test_extensions_for_language() {
        let exts = extensions_for_language("rust");
        assert!(exts.contains(&"rs"));

        let js_exts = extensions_for_language("javascript");
        assert!(js_exts.contains(&"js"));
        assert!(js_exts.contains(&"jsx"));
    }
}
