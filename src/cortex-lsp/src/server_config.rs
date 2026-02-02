//! LSP server configurations for various languages.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Configuration for an LSP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspServerConfig {
    /// Server identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Command to start the server.
    pub command: Vec<String>,
    /// File extensions this server handles.
    pub extensions: Vec<String>,
    /// Language IDs (for LSP).
    pub language_ids: Vec<String>,
    /// Environment variables.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Initialization options.
    #[serde(default)]
    pub init_options: serde_json::Value,
    /// Whether server is disabled.
    #[serde(default)]
    pub disabled: bool,
    /// Project root markers (files/dirs that indicate project root).
    #[serde(default)]
    pub root_markers: Vec<String>,
    /// Exclude markers (if present, skip this directory for this server).
    #[serde(default)]
    pub exclude_markers: Vec<String>,
    /// Workspace marker (indicates monorepo/workspace root).
    #[serde(default)]
    pub workspace_marker: Option<String>,
}

impl LspServerConfig {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            command: Vec::new(),
            extensions: Vec::new(),
            language_ids: Vec::new(),
            env: HashMap::new(),
            init_options: serde_json::Value::Null,
            disabled: false,
            root_markers: Vec::new(),
            exclude_markers: Vec::new(),
            workspace_marker: None,
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

    pub fn language_ids(mut self, ids: Vec<&str>) -> Self {
        self.language_ids = ids.into_iter().map(String::from).collect();
        self
    }

    pub fn root_markers(mut self, markers: Vec<&str>) -> Self {
        self.root_markers = markers.into_iter().map(String::from).collect();
        self
    }

    pub fn exclude_markers(mut self, markers: Vec<&str>) -> Self {
        self.exclude_markers = markers.into_iter().map(String::from).collect();
        self
    }

    pub fn workspace_marker(mut self, marker: &str) -> Self {
        self.workspace_marker = Some(marker.to_string());
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

lazy_static::lazy_static! {
    pub static ref BUILTIN_SERVERS: Vec<LspServerConfig> = vec![
        // TypeScript/JavaScript
        LspServerConfig::new("typescript", "TypeScript Language Server")
            .command(vec!["typescript-language-server", "--stdio"])
            .extensions(vec!["ts", "tsx", "js", "jsx", "mjs", "cjs"])
            .language_ids(vec!["typescript", "typescriptreact", "javascript", "javascriptreact"])
            .root_markers(vec!["tsconfig.json", "jsconfig.json", "package.json", ".git"])
            .exclude_markers(vec!["deno.json", "deno.jsonc"]),

        // Rust
        LspServerConfig::new("rust", "Rust Analyzer")
            .command(vec!["rust-analyzer"])
            .extensions(vec!["rs"])
            .language_ids(vec!["rust"])
            .root_markers(vec!["Cargo.toml", ".git"])
            .workspace_marker("Cargo.toml"),

        // Python
        LspServerConfig::new("python", "Pylsp")
            .command(vec!["pylsp"])
            .extensions(vec!["py", "pyi"])
            .language_ids(vec!["python"])
            .root_markers(vec!["pyproject.toml", "setup.py", "setup.cfg", "requirements.txt", "Pipfile", ".git"]),

        // Go
        LspServerConfig::new("go", "Gopls")
            .command(vec!["gopls"])
            .extensions(vec!["go", "mod"])
            .language_ids(vec!["go", "gomod"])
            .root_markers(vec!["go.mod", "go.work", ".git"])
            .workspace_marker("go.work"),

        // C/C++
        LspServerConfig::new("clangd", "Clangd")
            .command(vec!["clangd"])
            .extensions(vec!["c", "cpp", "cc", "cxx", "h", "hpp", "hxx"])
            .language_ids(vec!["c", "cpp"])
            .root_markers(vec!["compile_commands.json", "CMakeLists.txt", "Makefile", ".clangd", ".git"]),

        // Java
        LspServerConfig::new("java", "Eclipse JDT.LS")
            .command(vec!["jdtls"])
            .extensions(vec!["java"])
            .language_ids(vec!["java"])
            .root_markers(vec!["pom.xml", "build.gradle", "build.gradle.kts", "settings.gradle", ".git"])
            .workspace_marker("settings.gradle"),

        // Lua
        LspServerConfig::new("lua", "Lua Language Server")
            .command(vec!["lua-language-server"])
            .extensions(vec!["lua"])
            .language_ids(vec!["lua"])
            .root_markers(vec![".luarc.json", ".luacheckrc", ".git"]),

        // YAML
        LspServerConfig::new("yaml", "YAML Language Server")
            .command(vec!["yaml-language-server", "--stdio"])
            .extensions(vec!["yaml", "yml"])
            .language_ids(vec!["yaml"])
            .root_markers(vec![".git"]),

        // JSON
        LspServerConfig::new("json", "JSON Language Server")
            .command(vec!["vscode-json-language-server", "--stdio"])
            .extensions(vec!["json", "jsonc"])
            .language_ids(vec!["json", "jsonc"])
            .root_markers(vec!["package.json", ".git"]),

        // HTML
        LspServerConfig::new("html", "HTML Language Server")
            .command(vec!["vscode-html-language-server", "--stdio"])
            .extensions(vec!["html", "htm"])
            .language_ids(vec!["html"])
            .root_markers(vec!["package.json", ".git"]),

        // CSS
        LspServerConfig::new("css", "CSS Language Server")
            .command(vec!["vscode-css-language-server", "--stdio"])
            .extensions(vec!["css", "scss", "less"])
            .language_ids(vec!["css", "scss", "less"])
            .root_markers(vec!["package.json", ".git"]),

        // Bash
        LspServerConfig::new("bash", "Bash Language Server")
            .command(vec!["bash-language-server", "start"])
            .extensions(vec!["sh", "bash", "zsh"])
            .language_ids(vec!["shellscript"])
            .root_markers(vec![".git"]),

        // Dockerfile
        LspServerConfig::new("docker", "Docker Language Server")
            .command(vec!["docker-langserver", "--stdio"])
            .extensions(vec!["dockerfile"])
            .language_ids(vec!["dockerfile"])
            .root_markers(vec!["docker-compose.yml", "docker-compose.yaml", "Dockerfile", ".git"]),

        // Terraform
        LspServerConfig::new("terraform", "Terraform Language Server")
            .command(vec!["terraform-ls", "serve"])
            .extensions(vec!["tf", "tfvars"])
            .language_ids(vec!["terraform"])
            .root_markers(vec!["main.tf", "terraform.tfvars", ".terraform", ".git"]),

        // Zig
        LspServerConfig::new("zig", "Zig Language Server")
            .command(vec!["zls"])
            .extensions(vec!["zig"])
            .language_ids(vec!["zig"])
            .root_markers(vec!["build.zig", "build.zig.zon", ".git"]),

        // Elixir
        LspServerConfig::new("elixir", "ElixirLS")
            .command(vec!["elixir-ls"])
            .extensions(vec!["ex", "exs"])
            .language_ids(vec!["elixir"])
            .root_markers(vec!["mix.exs", ".git"]),

        // Vue (Volar)
        LspServerConfig::new("vue", "Vue Language Server (Volar)")
            .command(vec!["vue-language-server", "--stdio"])
            .extensions(vec!["vue"])
            .language_ids(vec!["vue"])
            .root_markers(vec!["vite.config.js", "vite.config.ts", "nuxt.config.js", "nuxt.config.ts", "package.json", ".git"]),

        // Svelte
        LspServerConfig::new("svelte", "Svelte Language Server")
            .command(vec!["svelteserver", "--stdio"])
            .extensions(vec!["svelte"])
            .language_ids(vec!["svelte"])
            .root_markers(vec!["svelte.config.js", "vite.config.js", "vite.config.ts", "package.json", ".git"]),

        // Biome (JavaScript/TypeScript linter and formatter)
        LspServerConfig::new("biome", "Biome")
            .command(vec!["biome", "lsp-proxy"])
            .extensions(vec!["js", "jsx", "ts", "tsx", "json", "jsonc"])
            .language_ids(vec!["javascript", "javascriptreact", "typescript", "typescriptreact", "json", "jsonc"])
            .root_markers(vec!["biome.json", "biome.jsonc", "package.json", ".git"]),

        // ESLint
        LspServerConfig::new("eslint", "ESLint Language Server")
            .command(vec!["vscode-eslint-language-server", "--stdio"])
            .extensions(vec!["js", "jsx", "ts", "tsx", "mjs", "cjs"])
            .language_ids(vec!["javascript", "javascriptreact", "typescript", "typescriptreact"])
            .root_markers(vec![".eslintrc", ".eslintrc.js", ".eslintrc.json", "eslint.config.js", "package.json", ".git"]),

        // Astro
        LspServerConfig::new("astro", "Astro Language Server")
            .command(vec!["astro-ls", "--stdio"])
            .extensions(vec!["astro"])
            .language_ids(vec!["astro"])
            .root_markers(vec!["astro.config.mjs", "astro.config.ts", "package.json", ".git"]),

        // Tailwind CSS
        LspServerConfig::new("tailwindcss", "Tailwind CSS Language Server")
            .command(vec!["tailwindcss-language-server", "--stdio"])
            .extensions(vec!["css", "scss", "html", "vue", "jsx", "tsx"])
            .language_ids(vec!["css", "scss", "html", "vue", "javascriptreact", "typescriptreact"])
            .root_markers(vec!["tailwind.config.js", "tailwind.config.ts", "tailwind.config.cjs", "package.json", ".git"]),

        // GraphQL
        LspServerConfig::new("graphql", "GraphQL Language Server")
            .command(vec!["graphql-lsp", "server", "-m", "stream"])
            .extensions(vec!["graphql", "gql"])
            .language_ids(vec!["graphql"])
            .root_markers(vec![".graphqlrc", "graphql.config.js", "graphql.config.ts", "package.json", ".git"]),

        // Prisma
        LspServerConfig::new("prisma", "Prisma Language Server")
            .command(vec!["prisma-language-server", "--stdio"])
            .extensions(vec!["prisma"])
            .language_ids(vec!["prisma"])
            .root_markers(vec!["schema.prisma", "package.json", ".git"]),

        // SQL
        LspServerConfig::new("sql", "SQL Language Server")
            .command(vec!["sql-language-server", "up", "--method", "stdio"])
            .extensions(vec!["sql"])
            .language_ids(vec!["sql"])
            .root_markers(vec![".git"]),

        // Markdown
        LspServerConfig::new("markdown", "Marksman")
            .command(vec!["marksman", "server"])
            .extensions(vec!["md", "markdown"])
            .language_ids(vec!["markdown"])
            .root_markers(vec![".git"]),

        // TOML
        LspServerConfig::new("toml", "Taplo TOML Language Server")
            .command(vec!["taplo", "lsp", "stdio"])
            .extensions(vec!["toml"])
            .language_ids(vec!["toml"])
            .root_markers(vec!["Cargo.toml", "pyproject.toml", ".git"]),

        // Deno (TypeScript/JavaScript runtime with built-in LSP)
        LspServerConfig::new("deno", "Deno Language Server")
            .command(vec!["deno", "lsp"])
            .extensions(vec!["ts", "tsx", "js", "jsx"])
            .language_ids(vec!["typescript", "typescriptreact", "javascript", "javascriptreact"])
            .root_markers(vec!["deno.json", "deno.jsonc"])
            .exclude_markers(vec!["package.json"]),

        // C# (csharp-ls)
        // Auto-download: dotnet tool install -g csharp-ls
        LspServerConfig::new("csharp", "C# Language Server")
            .command(vec!["csharp-ls"])
            .extensions(vec!["cs"])
            .language_ids(vec!["csharp"])
            .root_markers(vec!["*.csproj", "*.sln", ".git"]),

        // F# (fsautocomplete)
        // Auto-download: dotnet tool install -g fsautocomplete
        LspServerConfig::new("fsharp", "F# Language Server")
            .command(vec!["fsautocomplete"])
            .extensions(vec!["fs", "fsx", "fsi"])
            .language_ids(vec!["fsharp"])
            .root_markers(vec!["*.fsproj", "*.sln", ".git"]),

        // Swift (sourcekit-lsp)
        // Usually comes with Xcode
        LspServerConfig::new("swift", "Swift Language Server")
            .command(vec!["sourcekit-lsp"])
            .extensions(vec!["swift"])
            .language_ids(vec!["swift"])
            .root_markers(vec!["Package.swift", "*.xcodeproj", ".git"]),

        // PHP (intelephense)
        // Auto-download: npm install -g intelephense
        LspServerConfig::new("php", "PHP Intelephense")
            .command(vec!["intelephense", "--stdio"])
            .extensions(vec!["php"])
            .language_ids(vec!["php"])
            .root_markers(vec!["composer.json", "index.php", ".git"]),

        // Dart
        // Comes with Dart SDK
        LspServerConfig::new("dart", "Dart Language Server")
            .command(vec!["dart", "language-server", "--protocol=lsp"])
            .extensions(vec!["dart"])
            .language_ids(vec!["dart"])
            .root_markers(vec!["pubspec.yaml", ".git"]),

        // Ruby (solargraph)
        // Auto-download: gem install solargraph
        LspServerConfig::new("ruby", "Ruby Solargraph")
            .command(vec!["solargraph", "stdio"])
            .extensions(vec!["rb", "rake"])
            .language_ids(vec!["ruby"])
            .root_markers(vec!["Gemfile", ".ruby-version", ".git"]),

        // OCaml (ocamllsp)
        LspServerConfig::new("ocaml", "OCaml Language Server")
            .command(vec!["ocamllsp"])
            .extensions(vec!["ml", "mli"])
            .language_ids(vec!["ocaml", "ocaml.interface"])
            .root_markers(vec!["dune-project", "*.opam", ".git"]),

        // LaTeX (texlab)
        // Auto-download: GitHub releases
        LspServerConfig::new("latex", "Texlab")
            .command(vec!["texlab"])
            .extensions(vec!["tex", "bib", "sty"])
            .language_ids(vec!["latex", "bibtex"])
            .root_markers(vec!["*.tex", "latexmkrc", ".latexmkrc", ".git"]),

        // Gleam
        LspServerConfig::new("gleam", "Gleam Language Server")
            .command(vec!["gleam", "lsp"])
            .extensions(vec!["gleam"])
            .language_ids(vec!["gleam"])
            .root_markers(vec!["gleam.toml", ".git"]),

        // Clojure (clojure-lsp)
        // Auto-download: GitHub releases
        LspServerConfig::new("clojure", "Clojure LSP")
            .command(vec!["clojure-lsp"])
            .extensions(vec!["clj", "cljs", "cljc", "edn"])
            .language_ids(vec!["clojure", "clojurescript"])
            .root_markers(vec!["deps.edn", "project.clj", "shadow-cljs.edn", ".git"]),

        // Nix (nixd)
        LspServerConfig::new("nix", "Nixd")
            .command(vec!["nixd"])
            .extensions(vec!["nix"])
            .language_ids(vec!["nix"])
            .root_markers(vec!["flake.nix", "default.nix", "shell.nix", ".git"]),

        // Typst (tinymist)
        // Auto-download: GitHub releases
        LspServerConfig::new("typst", "Tinymist")
            .command(vec!["tinymist"])
            .extensions(vec!["typ"])
            .language_ids(vec!["typst"])
            .root_markers(vec!["*.typ", ".git"]),

        // Haskell (haskell-language-server)
        // Auto-download: ghcup
        LspServerConfig::new("haskell", "Haskell Language Server")
            .command(vec!["haskell-language-server-wrapper", "--lsp"])
            .extensions(vec!["hs", "lhs"])
            .language_ids(vec!["haskell", "literate haskell"])
            .root_markers(vec!["*.cabal", "stack.yaml", "hie.yaml", "cabal.project", ".git"]),
    ];
}

pub fn find_server_for_file(path: &str) -> Option<&'static LspServerConfig> {
    BUILTIN_SERVERS.iter().find(|s| s.matches_file(path))
}

pub fn find_server_by_id(id: &str) -> Option<&'static LspServerConfig> {
    BUILTIN_SERVERS.iter().find(|s| s.id == id)
}
