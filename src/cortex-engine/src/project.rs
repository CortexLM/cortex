//! Project detection and analysis.
//!
//! Provides utilities for detecting project types, frameworks,
//! and generating relevant context.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Project type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ProjectType {
    /// Rust project (Cargo.toml).
    Rust,
    /// Node.js/JavaScript project (package.json).
    Node,
    /// Python project (pyproject.toml, setup.py, requirements.txt).
    Python,
    /// Go project (go.mod).
    Go,
    /// Java/Maven project (pom.xml).
    Maven,
    /// Java/Gradle project (build.gradle).
    Gradle,
    /// Ruby project (Gemfile).
    Ruby,
    /// PHP/Composer project (composer.json).
    Php,
    /// .NET project (*.csproj, *.sln).
    DotNet,
    /// Swift project (Package.swift).
    Swift,
    /// Kotlin project.
    Kotlin,
    /// C/C++ project (CMakeLists.txt, Makefile).
    Cpp,
    /// Zig project (build.zig).
    Zig,
    /// Elixir project (mix.exs).
    Elixir,
    /// Unknown project type.
    #[default]
    Unknown,
}

impl ProjectType {
    /// Detect project type from path.
    pub fn detect(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();

        // Check for marker files
        if path.join("Cargo.toml").exists() {
            return Self::Rust;
        }
        if path.join("package.json").exists() {
            return Self::Node;
        }
        if path.join("pyproject.toml").exists()
            || path.join("setup.py").exists()
            || path.join("requirements.txt").exists()
        {
            return Self::Python;
        }
        if path.join("go.mod").exists() {
            return Self::Go;
        }
        if path.join("pom.xml").exists() {
            return Self::Maven;
        }
        if path.join("build.gradle").exists() || path.join("build.gradle.kts").exists() {
            return Self::Gradle;
        }
        if path.join("Gemfile").exists() {
            return Self::Ruby;
        }
        if path.join("composer.json").exists() {
            return Self::Php;
        }
        if has_extension(path, "csproj") || has_extension(path, "sln") {
            return Self::DotNet;
        }
        if path.join("Package.swift").exists() {
            return Self::Swift;
        }
        if path.join("CMakeLists.txt").exists() || path.join("Makefile").exists() {
            return Self::Cpp;
        }
        if path.join("build.zig").exists() {
            return Self::Zig;
        }
        if path.join("mix.exs").exists() {
            return Self::Elixir;
        }

        Self::Unknown
    }

    /// Get the primary language for this project type.
    pub fn language(&self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::Node => "JavaScript/TypeScript",
            Self::Python => "Python",
            Self::Go => "Go",
            Self::Maven | Self::Gradle => "Java",
            Self::Ruby => "Ruby",
            Self::Php => "PHP",
            Self::DotNet => "C#",
            Self::Swift => "Swift",
            Self::Kotlin => "Kotlin",
            Self::Cpp => "C/C++",
            Self::Zig => "Zig",
            Self::Elixir => "Elixir",
            Self::Unknown => "Unknown",
        }
    }

    /// Get common source directories.
    pub fn source_dirs(&self) -> Vec<&'static str> {
        match self {
            Self::Rust => vec!["src", "examples", "benches"],
            Self::Node => vec!["src", "lib", "app", "pages", "components"],
            Self::Python => vec!["src", "lib", "app"],
            Self::Go => vec!["cmd", "pkg", "internal"],
            Self::Maven | Self::Gradle => vec!["src/main/java", "src/test/java"],
            Self::Ruby => vec!["lib", "app"],
            Self::Php => vec!["src", "app"],
            Self::DotNet => vec!["src"],
            Self::Swift => vec!["Sources"],
            Self::Kotlin => vec!["src/main/kotlin"],
            Self::Cpp => vec!["src", "include"],
            Self::Zig => vec!["src"],
            Self::Elixir => vec!["lib"],
            Self::Unknown => vec!["src"],
        }
    }

    /// Get test directories.
    pub fn test_dirs(&self) -> Vec<&'static str> {
        match self {
            Self::Rust => vec!["tests"],
            Self::Node => vec!["test", "tests", "__tests__", "spec"],
            Self::Python => vec!["tests", "test"],
            Self::Go => vec![], // Tests are alongside source
            Self::Maven | Self::Gradle => vec!["src/test/java"],
            Self::Ruby => vec!["test", "spec"],
            Self::Php => vec!["tests"],
            Self::DotNet => vec!["tests"],
            Self::Swift => vec!["Tests"],
            Self::Kotlin => vec!["src/test/kotlin"],
            Self::Cpp => vec!["test", "tests"],
            Self::Zig => vec!["test"],
            Self::Elixir => vec!["test"],
            Self::Unknown => vec!["test", "tests"],
        }
    }

    /// Get build command.
    pub fn build_command(&self) -> Option<&'static str> {
        match self {
            Self::Rust => Some("cargo build"),
            Self::Node => Some("npm run build"),
            Self::Python => Some("python -m build"),
            Self::Go => Some("go build ./..."),
            Self::Maven => Some("mvn compile"),
            Self::Gradle => Some("./gradlew build"),
            Self::Ruby => Some("bundle exec rake build"),
            Self::Php => Some("composer install"),
            Self::DotNet => Some("dotnet build"),
            Self::Swift => Some("swift build"),
            Self::Cpp => Some("cmake --build ."),
            Self::Zig => Some("zig build"),
            Self::Elixir => Some("mix compile"),
            _ => None,
        }
    }

    /// Get test command.
    pub fn test_command(&self) -> Option<&'static str> {
        match self {
            Self::Rust => Some("cargo test"),
            Self::Node => Some("npm test"),
            Self::Python => Some("pytest"),
            Self::Go => Some("go test ./..."),
            Self::Maven => Some("mvn test"),
            Self::Gradle => Some("./gradlew test"),
            Self::Ruby => Some("bundle exec rspec"),
            Self::Php => Some("./vendor/bin/phpunit"),
            Self::DotNet => Some("dotnet test"),
            Self::Swift => Some("swift test"),
            Self::Cpp => Some("ctest"),
            Self::Zig => Some("zig build test"),
            Self::Elixir => Some("mix test"),
            _ => None,
        }
    }

    /// Get lint command.
    pub fn lint_command(&self) -> Option<&'static str> {
        match self {
            Self::Rust => Some("cargo clippy"),
            Self::Node => Some("npm run lint"),
            Self::Python => Some("ruff check ."),
            Self::Go => Some("golangci-lint run"),
            Self::Maven => Some("mvn checkstyle:check"),
            Self::Ruby => Some("bundle exec rubocop"),
            Self::Php => Some("./vendor/bin/phpcs"),
            Self::DotNet => Some("dotnet format --verify-no-changes"),
            Self::Swift => Some("swiftlint"),
            Self::Elixir => Some("mix credo"),
            _ => None,
        }
    }
}

/// Check if directory has file with extension.
fn has_extension(path: &Path, ext: &str) -> bool {
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            if entry.path().extension().map(|e| e == ext).unwrap_or(false) {
                return true;
            }
        }
    }
    false
}

/// Framework detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Framework {
    // JavaScript/TypeScript
    React,
    Next,
    Vue,
    Nuxt,
    Angular,
    Svelte,
    Express,
    Fastify,
    Nest,

    // Python
    Django,
    Flask,
    FastApi,
    Pyramid,

    // Ruby
    Rails,
    Sinatra,

    // Rust
    Actix,
    Axum,
    Rocket,
    Tauri,

    // Go
    Gin,
    Echo,
    Fiber,

    // PHP
    Laravel,
    Symfony,

    // Other
    Spring,
    DotNetCore,

    Unknown,
}

impl Framework {
    /// Detect framework from project.
    pub fn detect(path: impl AsRef<Path>, project_type: ProjectType) -> Self {
        let path = path.as_ref();

        match project_type {
            ProjectType::Node => Self::detect_node_framework(path),
            ProjectType::Python => Self::detect_python_framework(path),
            ProjectType::Ruby => Self::detect_ruby_framework(path),
            ProjectType::Rust => Self::detect_rust_framework(path),
            ProjectType::Go => Self::detect_go_framework(path),
            ProjectType::Php => Self::detect_php_framework(path),
            _ => Self::Unknown,
        }
    }

    fn detect_node_framework(path: &Path) -> Self {
        let package_json = path.join("package.json");
        if let Ok(content) = std::fs::read_to_string(&package_json) {
            if content.contains("\"next\"") {
                return Self::Next;
            }
            if content.contains("\"nuxt\"") {
                return Self::Nuxt;
            }
            if content.contains("\"react\"") {
                return Self::React;
            }
            if content.contains("\"vue\"") {
                return Self::Vue;
            }
            if content.contains("\"@angular/core\"") {
                return Self::Angular;
            }
            if content.contains("\"svelte\"") {
                return Self::Svelte;
            }
            if content.contains("\"@nestjs/core\"") {
                return Self::Nest;
            }
            if content.contains("\"fastify\"") {
                return Self::Fastify;
            }
            if content.contains("\"express\"") {
                return Self::Express;
            }
        }
        Self::Unknown
    }

    fn detect_python_framework(path: &Path) -> Self {
        // Check pyproject.toml and requirements.txt
        for file in ["pyproject.toml", "requirements.txt", "setup.py"] {
            if let Ok(content) = std::fs::read_to_string(path.join(file)) {
                if content.contains("django") || content.contains("Django") {
                    return Self::Django;
                }
                if content.contains("fastapi") || content.contains("FastAPI") {
                    return Self::FastApi;
                }
                if content.contains("flask") || content.contains("Flask") {
                    return Self::Flask;
                }
                if content.contains("pyramid") {
                    return Self::Pyramid;
                }
            }
        }
        Self::Unknown
    }

    fn detect_ruby_framework(path: &Path) -> Self {
        if path.join("config/application.rb").exists() {
            return Self::Rails;
        }
        if let Ok(content) = std::fs::read_to_string(path.join("Gemfile"))
            && content.contains("sinatra")
        {
            return Self::Sinatra;
        }
        Self::Unknown
    }

    fn detect_rust_framework(path: &Path) -> Self {
        if let Ok(content) = std::fs::read_to_string(path.join("Cargo.toml")) {
            if content.contains("tauri") {
                return Self::Tauri;
            }
            if content.contains("actix-web") {
                return Self::Actix;
            }
            if content.contains("axum") {
                return Self::Axum;
            }
            if content.contains("rocket") {
                return Self::Rocket;
            }
        }
        Self::Unknown
    }

    fn detect_go_framework(path: &Path) -> Self {
        if let Ok(content) = std::fs::read_to_string(path.join("go.mod")) {
            if content.contains("github.com/gin-gonic/gin") {
                return Self::Gin;
            }
            if content.contains("github.com/labstack/echo") {
                return Self::Echo;
            }
            if content.contains("github.com/gofiber/fiber") {
                return Self::Fiber;
            }
        }
        Self::Unknown
    }

    fn detect_php_framework(path: &Path) -> Self {
        if path.join("artisan").exists() {
            return Self::Laravel;
        }
        if path.join("bin/console").exists() {
            return Self::Symfony;
        }
        Self::Unknown
    }
}

/// Project information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInfo {
    /// Project root.
    pub root: PathBuf,
    /// Project name.
    pub name: Option<String>,
    /// Project type.
    pub project_type: ProjectType,
    /// Detected framework.
    pub framework: Framework,
    /// Source files count.
    pub source_files: u32,
    /// Test files count.
    pub test_files: u32,
    /// Dependencies count.
    pub dependencies: u32,
    /// Has git repository.
    pub has_git: bool,
    /// Has CI configuration.
    pub has_ci: bool,
    /// Has Docker configuration.
    pub has_docker: bool,
    /// Entry points.
    pub entry_points: Vec<PathBuf>,
    /// Important files.
    pub important_files: Vec<PathBuf>,
}

impl ProjectInfo {
    /// Analyze a project.
    pub fn analyze(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let project_type = ProjectType::detect(path);
        let framework = Framework::detect(path, project_type);

        let name = detect_project_name(path, project_type);
        let has_git = path.join(".git").exists();
        let has_ci = path.join(".github/workflows").exists()
            || path.join(".gitlab-ci.yml").exists()
            || path.join(".circleci").exists();
        let has_docker =
            path.join("Dockerfile").exists() || path.join("docker-compose.yml").exists();

        let mut source_files = 0u32;
        let mut test_files = 0u32;

        // Count files
        for src_dir in project_type.source_dirs() {
            let src_path = path.join(src_dir);
            if src_path.exists() {
                source_files += count_source_files(&src_path, project_type);
            }
        }

        for test_dir in project_type.test_dirs() {
            let test_path = path.join(test_dir);
            if test_path.exists() {
                test_files += count_source_files(&test_path, project_type);
            }
        }

        let dependencies = count_dependencies(path, project_type);
        let entry_points = find_entry_points(path, project_type);
        let important_files = find_important_files(path);

        Ok(Self {
            root: path.to_path_buf(),
            name,
            project_type,
            framework,
            source_files,
            test_files,
            dependencies,
            has_git,
            has_ci,
            has_docker,
            entry_points,
            important_files,
        })
    }

    /// Get a context summary for AI.
    pub fn context_summary(&self) -> String {
        let mut parts = Vec::new();

        if let Some(ref name) = self.name {
            parts.push(format!("Project: {name}"));
        }

        parts.push(format!(
            "Type: {} ({})",
            self.project_type.language(),
            format!("{:?}", self.project_type)
        ));

        if self.framework != Framework::Unknown {
            parts.push(format!("Framework: {:?}", self.framework));
        }

        parts.push(format!(
            "Files: {} source, {} test",
            self.source_files, self.test_files
        ));
        parts.push(format!("Dependencies: {}", self.dependencies));

        let mut features = Vec::new();
        if self.has_git {
            features.push("git");
        }
        if self.has_ci {
            features.push("CI");
        }
        if self.has_docker {
            features.push("Docker");
        }
        if !features.is_empty() {
            parts.push(format!("Features: {}", features.join(", ")));
        }

        parts.join("\n")
    }
}

/// Detect project name.
fn detect_project_name(path: &Path, project_type: ProjectType) -> Option<String> {
    match project_type {
        ProjectType::Rust => {
            let cargo = path.join("Cargo.toml");
            if let Ok(content) = std::fs::read_to_string(&cargo) {
                for line in content.lines() {
                    if line.starts_with("name") {
                        return line
                            .split('=')
                            .nth(1)
                            .map(|s| s.trim().trim_matches('"').to_string());
                    }
                }
            }
        }
        ProjectType::Node => {
            let package = path.join("package.json");
            if let Ok(content) = std::fs::read_to_string(&package)
                && let Ok(json) = serde_json::from_str::<serde_json::Value>(&content)
            {
                return json
                    .get("name")
                    .and_then(|n| n.as_str())
                    .map(std::string::ToString::to_string);
            }
        }
        ProjectType::Python => {
            let pyproject = path.join("pyproject.toml");
            if let Ok(content) = std::fs::read_to_string(&pyproject) {
                for line in content.lines() {
                    if line.starts_with("name") {
                        return line
                            .split('=')
                            .nth(1)
                            .map(|s| s.trim().trim_matches('"').to_string());
                    }
                }
            }
        }
        _ => {}
    }

    // Fallback to directory name
    path.file_name().map(|n| n.to_string_lossy().to_string())
}

/// Count source files.
fn count_source_files(path: &Path, project_type: ProjectType) -> u32 {
    let extensions: Vec<&str> = match project_type {
        ProjectType::Rust => vec!["rs"],
        ProjectType::Node => vec!["js", "ts", "jsx", "tsx"],
        ProjectType::Python => vec!["py"],
        ProjectType::Go => vec!["go"],
        ProjectType::Maven | ProjectType::Gradle => vec!["java"],
        ProjectType::Ruby => vec!["rb"],
        ProjectType::Php => vec!["php"],
        ProjectType::DotNet => vec!["cs"],
        ProjectType::Swift => vec!["swift"],
        ProjectType::Kotlin => vec!["kt", "kts"],
        ProjectType::Cpp => vec!["c", "cpp", "cc", "h", "hpp"],
        ProjectType::Zig => vec!["zig"],
        ProjectType::Elixir => vec!["ex", "exs"],
        ProjectType::Unknown => vec![],
    };

    let mut count = 0u32;
    for entry in walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        if let Some(ext) = entry.path().extension()
            && extensions.contains(&ext.to_str().unwrap_or(""))
        {
            count += 1;
        }
    }
    count
}

/// Count dependencies.
fn count_dependencies(path: &Path, project_type: ProjectType) -> u32 {
    match project_type {
        ProjectType::Node => {
            if let Ok(content) = std::fs::read_to_string(path.join("package.json"))
                && let Ok(json) = serde_json::from_str::<serde_json::Value>(&content)
            {
                let deps = json
                    .get("dependencies")
                    .and_then(|d| d.as_object())
                    .map(serde_json::Map::len)
                    .unwrap_or(0);
                let dev_deps = json
                    .get("devDependencies")
                    .and_then(|d| d.as_object())
                    .map(serde_json::Map::len)
                    .unwrap_or(0);
                return (deps + dev_deps) as u32;
            }
        }
        ProjectType::Rust => {
            if let Ok(content) = std::fs::read_to_string(path.join("Cargo.toml")) {
                return content
                    .lines()
                    .filter(|l| {
                        l.trim().starts_with(|c: char| c.is_alphanumeric())
                            && l.contains('=')
                            && !l.starts_with('[')
                    })
                    .count() as u32;
            }
        }
        _ => {}
    }
    0
}

/// Find entry points.
fn find_entry_points(path: &Path, project_type: ProjectType) -> Vec<PathBuf> {
    let candidates: Vec<&str> = match project_type {
        ProjectType::Rust => vec!["src/main.rs", "src/lib.rs"],
        ProjectType::Node => vec![
            "src/index.ts",
            "src/index.js",
            "index.js",
            "app.js",
            "server.js",
        ],
        ProjectType::Python => vec!["main.py", "app.py", "manage.py", "__main__.py"],
        ProjectType::Go => vec!["main.go", "cmd/main.go"],
        _ => vec![],
    };

    candidates
        .iter()
        .map(|c| path.join(c))
        .filter(|p| p.exists())
        .collect()
}

/// Find important files.
fn find_important_files(path: &Path) -> Vec<PathBuf> {
    let files = [
        "README.md",
        "CONTRIBUTING.md",
        "LICENSE",
        ".env.example",
        "Makefile",
        "justfile",
    ];

    files
        .iter()
        .map(|f| path.join(f))
        .filter(|p| p.exists())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_project_type_language() {
        assert_eq!(ProjectType::Rust.language(), "Rust");
        assert_eq!(ProjectType::Node.language(), "JavaScript/TypeScript");
    }

    #[test]
    fn test_project_type_commands() {
        assert_eq!(ProjectType::Rust.build_command(), Some("cargo build"));
        assert_eq!(ProjectType::Rust.test_command(), Some("cargo test"));
    }

    #[test]
    fn test_framework_unknown() {
        assert_eq!(
            Framework::detect("/nonexistent", ProjectType::Unknown),
            Framework::Unknown
        );
    }
}
