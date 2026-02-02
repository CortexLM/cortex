//! Init command implementation.
//!
//! The `/init` command creates or updates an AGENTS.md file in the current directory,
//! providing project-specific instructions for AI agents.
//!
//! # Atomic File Writes
//!
//! All file operations use atomic writes to prevent corruption:
//! - Writes to a temporary file first
//! - Syncs to disk
//! - Atomically renames to the target path
//!
//! # Hierarchy Support
//!
//! The init command respects the AGENTS.md hierarchy:
//! - Global: `~/.config/cortex/AGENTS.md`
//! - Project: `<project>/AGENTS.md`
//! - Local: `<project>/.cortex/AGENTS.md`

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::builtin::atomic::{AtomicWriteError, atomic_write_str};
use crate::builtin::templates::{
    AGENTS_MD_MINIMAL_TEMPLATE, AGENTS_MD_TEMPLATE, GO_PROJECT_DEFAULTS, NODE_PROJECT_DEFAULTS,
    PYTHON_PROJECT_DEFAULTS, RUST_PROJECT_DEFAULTS,
};

/// Errors that can occur during init command execution.
#[derive(Debug, Error)]
pub enum InitError {
    /// Failed to read directory.
    #[error("Failed to read directory: {0}")]
    ReadDir(#[from] std::io::Error),

    /// Failed to read file.
    #[error("Failed to read file '{path}': {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Failed to write file.
    #[error("Failed to write file '{path}': {source}")]
    WriteFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Failed to write file atomically.
    #[error("Atomic write failed: {0}")]
    AtomicWrite(#[from] AtomicWriteError),

    /// Failed to parse configuration file.
    #[error("Failed to parse {file_type}: {message}")]
    ParseError { file_type: String, message: String },
}

/// Result of executing the init command.
#[derive(Debug, Clone)]
pub enum InitResult {
    /// Successfully created a new AGENTS.md file.
    Created(PathBuf),
    /// AGENTS.md already exists at the path.
    AlreadyExists(PathBuf),
    /// Successfully updated an existing AGENTS.md file.
    Updated(PathBuf),
}

impl InitResult {
    /// Get the path to the AGENTS.md file.
    pub fn path(&self) -> &Path {
        match self {
            Self::Created(p) | Self::AlreadyExists(p) | Self::Updated(p) => p,
        }
    }

    /// Check if the file was created.
    pub fn is_created(&self) -> bool {
        matches!(self, Self::Created(_))
    }

    /// Check if the file already existed.
    pub fn already_exists(&self) -> bool {
        matches!(self, Self::AlreadyExists(_))
    }

    /// Get a user-friendly message describing the result.
    pub fn message(&self) -> String {
        match self {
            Self::Created(path) => {
                format!(
                    "Created AGENTS.md at {}\n\nEdit this file to provide project-specific instructions to the AI agent.",
                    path.display()
                )
            }
            Self::AlreadyExists(path) => {
                format!(
                    "AGENTS.md already exists at {}\nUse --force to overwrite or edit the file manually.",
                    path.display()
                )
            }
            Self::Updated(path) => {
                format!("Updated AGENTS.md at {}", path.display())
            }
        }
    }
}

/// Detected project type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProjectType {
    /// Rust project (Cargo.toml).
    Rust,
    /// Node.js project (package.json).
    Node,
    /// Python project (pyproject.toml, setup.py, requirements.txt).
    Python,
    /// Go project (go.mod).
    Go,
    /// Unknown or mixed project type.
    #[default]
    Unknown,
}

impl ProjectType {
    /// Get the display name for this project type.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::Node => "Node.js/TypeScript",
            Self::Python => "Python",
            Self::Go => "Go",
            Self::Unknown => "Unknown",
        }
    }
}

impl std::fmt::Display for ProjectType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

/// Information about the project detected by analysis.
#[derive(Debug, Clone, Default)]
pub struct ProjectInfo {
    /// Detected project type.
    pub project_type: ProjectType,
    /// Project name (from manifest or directory name).
    pub project_name: String,
    /// Project description (from manifest).
    pub description: Option<String>,
    /// Build command.
    pub build_command: String,
    /// Test command.
    pub test_command: String,
    /// Run command.
    pub run_command: String,
    /// Important files found in the project.
    pub key_files: Vec<KeyFile>,
    /// Detected dependencies/frameworks.
    pub frameworks: Vec<String>,
}

/// A key file in the project.
#[derive(Debug, Clone)]
pub struct KeyFile {
    /// Relative path to the file.
    pub path: PathBuf,
    /// Description of the file's purpose.
    pub description: String,
}

impl KeyFile {
    /// Create a new key file entry.
    pub fn new(path: impl Into<PathBuf>, description: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            description: description.into(),
        }
    }
}

/// Options for the init command.
#[derive(Debug, Clone, Default)]
pub struct InitOptions {
    /// Force overwrite if file exists.
    pub force: bool,
    /// Use AI to generate a more detailed AGENTS.md.
    pub use_ai: bool,
    /// Custom template to use instead of defaults.
    pub custom_template: Option<String>,
}

/// The init command handler.
#[derive(Debug, Clone, Default)]
pub struct InitCommand {
    options: InitOptions,
}

impl InitCommand {
    /// Create a new init command handler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create with custom options.
    pub fn with_options(options: InitOptions) -> Self {
        Self { options }
    }

    /// Set whether to force overwrite.
    pub fn force(mut self, force: bool) -> Self {
        self.options.force = force;
        self
    }

    /// Execute the init command in the given directory.
    ///
    /// Uses atomic file writes to ensure the file is never in a corrupted state.
    pub fn execute(&self, cwd: &Path) -> Result<InitResult, InitError> {
        let agents_path = cwd.join("AGENTS.md");
        let file_existed = agents_path.exists();

        // Check if file already exists
        if file_existed && !self.options.force {
            return Ok(InitResult::AlreadyExists(agents_path));
        }

        // Analyze the project
        let project_info = self.analyze_project(cwd)?;

        // Generate content
        let content = self.generate_content(&project_info);

        // Write the file atomically to prevent corruption
        atomic_write_str(&agents_path, &content)?;

        if self.options.force && file_existed {
            Ok(InitResult::Updated(agents_path))
        } else {
            Ok(InitResult::Created(agents_path))
        }
    }

    /// Execute the init command asynchronously.
    ///
    /// Uses atomic file writes to ensure the file is never in a corrupted state.
    pub async fn execute_async(&self, cwd: &Path) -> Result<InitResult, InitError> {
        let agents_path = cwd.join("AGENTS.md");
        let file_existed = tokio::fs::try_exists(&agents_path).await.unwrap_or(false);

        // Check if file already exists
        if file_existed && !self.options.force {
            return Ok(InitResult::AlreadyExists(agents_path));
        }

        // Analyze the project
        let project_info = self.analyze_project_async(cwd).await?;

        // Generate content
        let content = self.generate_content(&project_info);

        // Write the file atomically to prevent corruption
        // Note: atomic_write_str is sync but fast enough for small files
        atomic_write_str(&agents_path, &content)?;

        if self.options.force && file_existed {
            Ok(InitResult::Updated(agents_path))
        } else {
            Ok(InitResult::Created(agents_path))
        }
    }

    /// Analyze the project to detect type, structure, and configuration.
    pub fn analyze_project(&self, cwd: &Path) -> Result<ProjectInfo, InitError> {
        let mut info = ProjectInfo {
            project_name: cwd
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("project")
                .to_string(),
            ..Default::default()
        };

        // Detect project type and gather info
        if cwd.join("Cargo.toml").exists() {
            info.project_type = ProjectType::Rust;
            self.analyze_rust_project(cwd, &mut info)?;
        } else if cwd.join("package.json").exists() {
            info.project_type = ProjectType::Node;
            self.analyze_node_project(cwd, &mut info)?;
        } else if cwd.join("pyproject.toml").exists()
            || cwd.join("setup.py").exists()
            || cwd.join("requirements.txt").exists()
        {
            info.project_type = ProjectType::Python;
            self.analyze_python_project(cwd, &mut info)?;
        } else if cwd.join("go.mod").exists() {
            info.project_type = ProjectType::Go;
            self.analyze_go_project(cwd, &mut info)?;
        }

        // Find key files if not already populated
        if info.key_files.is_empty() {
            info.key_files = self.find_key_files(cwd)?;
        }

        // Set default commands based on project type
        self.set_default_commands(&mut info);

        Ok(info)
    }

    /// Analyze the project asynchronously.
    pub async fn analyze_project_async(&self, cwd: &Path) -> Result<ProjectInfo, InitError> {
        // For now, delegate to sync version
        // In a real implementation, you'd use async file operations
        self.analyze_project(cwd)
    }

    /// Analyze a Rust project.
    fn analyze_rust_project(&self, cwd: &Path, info: &mut ProjectInfo) -> Result<(), InitError> {
        let cargo_path = cwd.join("Cargo.toml");

        if let Ok(content) = std::fs::read_to_string(&cargo_path) {
            // Simple parsing - in production you'd use toml crate
            if let Some(name) = extract_toml_value(&content, "name") {
                info.project_name = name;
            }
            if let Some(desc) = extract_toml_value(&content, "description") {
                info.description = Some(desc);
            }

            // Detect frameworks from dependencies
            if content.contains("tokio") {
                info.frameworks.push("tokio (async runtime)".to_string());
            }
            if content.contains("serde") {
                info.frameworks.push("serde (serialization)".to_string());
            }
            if content.contains("axum") || content.contains("actix") || content.contains("rocket") {
                info.frameworks.push("web framework".to_string());
            }
        }

        // Add Rust-specific key files
        info.key_files.push(KeyFile::new(
            "Cargo.toml",
            "Project manifest and dependencies",
        ));

        if cwd.join("src/main.rs").exists() {
            info.key_files
                .push(KeyFile::new("src/main.rs", "Application entry point"));
        }
        if cwd.join("src/lib.rs").exists() {
            info.key_files
                .push(KeyFile::new("src/lib.rs", "Library root module"));
        }

        Ok(())
    }

    /// Analyze a Node.js project.
    fn analyze_node_project(&self, cwd: &Path, info: &mut ProjectInfo) -> Result<(), InitError> {
        let package_path = cwd.join("package.json");

        if let Ok(content) = std::fs::read_to_string(&package_path) {
            // Simple parsing - in production you'd use serde_json
            if let Some(name) = extract_json_value(&content, "name") {
                info.project_name = name;
            }
            if let Some(desc) = extract_json_value(&content, "description") {
                info.description = Some(desc);
            }

            // Detect frameworks
            if content.contains("\"react\"") {
                info.frameworks.push("React".to_string());
            }
            if content.contains("\"next\"") {
                info.frameworks.push("Next.js".to_string());
            }
            if content.contains("\"express\"") {
                info.frameworks.push("Express".to_string());
            }
            if content.contains("\"typescript\"") {
                info.frameworks.push("TypeScript".to_string());
            }
        }

        // Add Node-specific key files
        info.key_files.push(KeyFile::new(
            "package.json",
            "Project manifest and dependencies",
        ));

        if cwd.join("tsconfig.json").exists() {
            info.key_files
                .push(KeyFile::new("tsconfig.json", "TypeScript configuration"));
        }

        Ok(())
    }

    /// Analyze a Python project.
    fn analyze_python_project(&self, cwd: &Path, info: &mut ProjectInfo) -> Result<(), InitError> {
        // Check pyproject.toml first (modern Python)
        let pyproject_path = cwd.join("pyproject.toml");
        if pyproject_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&pyproject_path) {
                if let Some(name) = extract_toml_value(&content, "name") {
                    info.project_name = name;
                }
                if let Some(desc) = extract_toml_value(&content, "description") {
                    info.description = Some(desc);
                }

                // Detect frameworks
                if content.contains("django") {
                    info.frameworks.push("Django".to_string());
                }
                if content.contains("fastapi") {
                    info.frameworks.push("FastAPI".to_string());
                }
                if content.contains("flask") {
                    info.frameworks.push("Flask".to_string());
                }
            }
            info.key_files
                .push(KeyFile::new("pyproject.toml", "Project configuration"));
        }

        if cwd.join("requirements.txt").exists() {
            info.key_files
                .push(KeyFile::new("requirements.txt", "Project dependencies"));
        }

        Ok(())
    }

    /// Analyze a Go project.
    fn analyze_go_project(&self, cwd: &Path, info: &mut ProjectInfo) -> Result<(), InitError> {
        let go_mod_path = cwd.join("go.mod");

        if let Ok(content) = std::fs::read_to_string(&go_mod_path) {
            // Extract module name
            if let Some(line) = content.lines().find(|l| l.starts_with("module ")) {
                let module_name = line.trim_start_matches("module ").trim();
                // Use the last part of the module path as project name
                info.project_name = module_name
                    .rsplit('/')
                    .next()
                    .unwrap_or(module_name)
                    .to_string();
            }

            // Detect frameworks
            if content.contains("gin-gonic") {
                info.frameworks.push("Gin".to_string());
            }
            if content.contains("gorilla/mux") {
                info.frameworks.push("Gorilla Mux".to_string());
            }
        }

        info.key_files
            .push(KeyFile::new("go.mod", "Module definition and dependencies"));

        if cwd.join("main.go").exists() {
            info.key_files
                .push(KeyFile::new("main.go", "Application entry point"));
        }

        Ok(())
    }

    /// Find key files in the project directory.
    fn find_key_files(&self, cwd: &Path) -> Result<Vec<KeyFile>, InitError> {
        let mut key_files = Vec::new();

        // Common important files
        let common_files = [
            ("README.md", "Project documentation"),
            ("LICENSE", "License information"),
            (".gitignore", "Git ignore patterns"),
            ("Makefile", "Build automation"),
            ("Dockerfile", "Container configuration"),
            ("docker-compose.yml", "Container orchestration"),
            (".env.example", "Environment variables template"),
        ];

        for (file, desc) in common_files {
            if cwd.join(file).exists() {
                key_files.push(KeyFile::new(file, desc));
            }
        }

        // Look for src directory structure
        let src_dir = cwd.join("src");
        if src_dir.is_dir()
            && let Ok(entries) = std::fs::read_dir(&src_dir)
        {
            for entry in entries.flatten().take(5) {
                if let Some(name) = entry.file_name().to_str()
                    && (name.ends_with(".rs")
                        || name.ends_with(".ts")
                        || name.ends_with(".py")
                        || name.ends_with(".go"))
                {
                    key_files.push(KeyFile::new(
                        format!("src/{name}"),
                        "Source file".to_string(),
                    ));
                }
            }
        }

        Ok(key_files)
    }

    /// Set default commands based on project type.
    fn set_default_commands(&self, info: &mut ProjectInfo) {
        match info.project_type {
            ProjectType::Rust => {
                info.build_command = RUST_PROJECT_DEFAULTS.build_command.to_string();
                info.test_command = RUST_PROJECT_DEFAULTS.test_command.to_string();
                info.run_command = RUST_PROJECT_DEFAULTS.run_command.to_string();
            }
            ProjectType::Node => {
                info.build_command = NODE_PROJECT_DEFAULTS.build_command.to_string();
                info.test_command = NODE_PROJECT_DEFAULTS.test_command.to_string();
                info.run_command = NODE_PROJECT_DEFAULTS.run_command.to_string();
            }
            ProjectType::Python => {
                info.build_command = PYTHON_PROJECT_DEFAULTS.build_command.to_string();
                info.test_command = PYTHON_PROJECT_DEFAULTS.test_command.to_string();
                info.run_command = PYTHON_PROJECT_DEFAULTS
                    .run_command
                    .replace("{module_name}", &info.project_name);
            }
            ProjectType::Go => {
                info.build_command = GO_PROJECT_DEFAULTS.build_command.to_string();
                info.test_command = GO_PROJECT_DEFAULTS.test_command.to_string();
                info.run_command = GO_PROJECT_DEFAULTS.run_command.to_string();
            }
            ProjectType::Unknown => {
                info.build_command = "# Add build command".to_string();
                info.test_command = "# Add test command".to_string();
                info.run_command = "# Add run command".to_string();
            }
        }
    }

    /// Generate the AGENTS.md content based on project info.
    pub fn generate_content(&self, info: &ProjectInfo) -> String {
        // Use custom template if provided
        if let Some(ref template) = self.options.custom_template {
            return self.apply_template(template, info);
        }

        // Use minimal template for unknown projects
        if info.project_type == ProjectType::Unknown {
            return AGENTS_MD_MINIMAL_TEMPLATE.to_string();
        }

        self.apply_template(AGENTS_MD_TEMPLATE, info)
    }

    /// Apply project info to a template.
    fn apply_template(&self, template: &str, info: &ProjectInfo) -> String {
        let defaults = match info.project_type {
            ProjectType::Rust => &RUST_PROJECT_DEFAULTS,
            ProjectType::Node => &NODE_PROJECT_DEFAULTS,
            ProjectType::Python => &PYTHON_PROJECT_DEFAULTS,
            ProjectType::Go => &GO_PROJECT_DEFAULTS,
            ProjectType::Unknown => &RUST_PROJECT_DEFAULTS, // Fallback, won't be used
        };

        let description = info
            .description
            .clone()
            .unwrap_or_else(|| format!("{} - {}", info.project_name, info.project_type));

        let architecture = if !info.frameworks.is_empty() {
            format!(
                "This project uses: {}\n\n<!-- Add more details about the architecture -->",
                info.frameworks.join(", ")
            )
        } else {
            "<!-- Describe the architecture, main components, and their relationships -->"
                .to_string()
        };

        let key_files = if info.key_files.is_empty() {
            defaults.format_key_files()
        } else {
            info.key_files
                .iter()
                .map(|kf| format!("- `{}` - {}", kf.path.display(), kf.description))
                .collect::<Vec<_>>()
                .join("\n")
        };

        template
            .replace("{project_description}", &description)
            .replace("{architecture_description}", &architecture)
            .replace("{key_files}", &key_files)
            .replace("{code_style_guidelines}", &defaults.format_code_style())
            .replace("{testing_guidelines}", &defaults.format_testing())
            .replace("{build_command}", &info.build_command)
            .replace("{test_command}", &info.test_command)
            .replace("{run_command}", &info.run_command)
            .replace("{project_type}", defaults.project_type)
            .replace("{focus_areas}", defaults.focus_areas)
            .replace("{things_to_avoid}", defaults.things_to_avoid)
    }
}

/// Simple helper to extract a value from TOML content.
fn extract_toml_value(content: &str, key: &str) -> Option<String> {
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with(key) && line.contains('=') {
            let value = line.split('=').nth(1)?.trim();
            // Remove quotes
            return Some(value.trim_matches('"').trim_matches('\'').to_string());
        }
    }
    None
}

/// Simple helper to extract a value from JSON content.
fn extract_json_value(content: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{key}\"");
    for line in content.lines() {
        if line.contains(&pattern) {
            // Find the value after the colon
            if let Some(colon_pos) = line.find(':') {
                let value_part = &line[colon_pos + 1..];
                let value = value_part.trim().trim_matches(',').trim().trim_matches('"');
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_rust_project() -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            r#"
[package]
name = "test-project"
version = "0.1.0"
description = "A test project"

[dependencies]
tokio = "1.0"
serde = "1.0"
"#,
        )
        .unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();
        dir
    }

    fn setup_node_project() -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{
  "name": "test-node-project",
  "version": "1.0.0",
  "description": "A test Node.js project",
  "dependencies": {
    "typescript": "^5.0.0",
    "express": "^4.18.0"
  }
}"#,
        )
        .unwrap();
        dir
    }

    #[test]
    fn test_analyze_rust_project() {
        let dir = setup_rust_project();
        let cmd = InitCommand::new();

        let info = cmd.analyze_project(dir.path()).unwrap();

        assert_eq!(info.project_type, ProjectType::Rust);
        assert_eq!(info.project_name, "test-project");
        assert_eq!(info.description, Some("A test project".to_string()));
        assert!(info.frameworks.iter().any(|f| f.contains("tokio")));
        assert!(info.frameworks.iter().any(|f| f.contains("serde")));
    }

    #[test]
    fn test_analyze_node_project() {
        let dir = setup_node_project();
        let cmd = InitCommand::new();

        let info = cmd.analyze_project(dir.path()).unwrap();

        assert_eq!(info.project_type, ProjectType::Node);
        assert_eq!(info.project_name, "test-node-project");
        assert!(info.frameworks.iter().any(|f| f.contains("TypeScript")));
        assert!(info.frameworks.iter().any(|f| f.contains("Express")));
    }

    #[test]
    fn test_execute_creates_file() {
        let dir = TempDir::new().unwrap();
        let cmd = InitCommand::new();

        let result = cmd.execute(dir.path()).unwrap();

        assert!(result.is_created());
        assert!(dir.path().join("AGENTS.md").exists());
    }

    #[test]
    fn test_execute_respects_existing_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "existing content").unwrap();

        let cmd = InitCommand::new();
        let result = cmd.execute(dir.path()).unwrap();

        assert!(result.already_exists());
        // Content should be unchanged
        let content = fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
        assert_eq!(content, "existing content");
    }

    #[test]
    fn test_execute_force_overwrites() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("AGENTS.md"), "existing content").unwrap();

        let cmd = InitCommand::new().force(true);
        let result = cmd.execute(dir.path()).unwrap();

        assert!(matches!(result, InitResult::Updated(_)));
        // Content should be changed
        let content = fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
        assert_ne!(content, "existing content");
    }

    #[test]
    fn test_generate_content_rust() {
        let dir = setup_rust_project();
        let cmd = InitCommand::new();

        let info = cmd.analyze_project(dir.path()).unwrap();
        let content = cmd.generate_content(&info);

        assert!(content.contains("# AGENTS.md"));
        assert!(content.contains("cargo build"));
        assert!(content.contains("cargo test"));
        assert!(content.contains("Rust"));
    }

    #[test]
    fn test_generate_content_unknown_project() {
        let dir = TempDir::new().unwrap();
        let cmd = InitCommand::new();

        let info = cmd.analyze_project(dir.path()).unwrap();
        let content = cmd.generate_content(&info);

        assert!(content.contains("# AGENTS.md"));
        // Should use minimal template
        assert!(content.contains("<!-- Describe your project here -->"));
    }

    #[test]
    fn test_extract_toml_value() {
        let toml = r#"
[package]
name = "my-project"
version = "1.0.0"
"#;
        assert_eq!(
            extract_toml_value(toml, "name"),
            Some("my-project".to_string())
        );
        assert_eq!(
            extract_toml_value(toml, "version"),
            Some("1.0.0".to_string())
        );
        assert_eq!(extract_toml_value(toml, "missing"), None);
    }

    #[test]
    fn test_extract_json_value() {
        let json = r#"{
  "name": "my-project",
  "version": "1.0.0"
}"#;
        assert_eq!(
            extract_json_value(json, "name"),
            Some("my-project".to_string())
        );
        assert_eq!(
            extract_json_value(json, "version"),
            Some("1.0.0".to_string())
        );
        assert_eq!(extract_json_value(json, "missing"), None);
    }

    #[test]
    fn test_project_type_display() {
        assert_eq!(ProjectType::Rust.display_name(), "Rust");
        assert_eq!(ProjectType::Node.display_name(), "Node.js/TypeScript");
        assert_eq!(ProjectType::Python.display_name(), "Python");
        assert_eq!(ProjectType::Go.display_name(), "Go");
        assert_eq!(ProjectType::Unknown.display_name(), "Unknown");
    }

    #[test]
    fn test_init_result_message() {
        let path = PathBuf::from("/test/AGENTS.md");

        let created = InitResult::Created(path.clone());
        assert!(created.message().contains("Created"));

        let exists = InitResult::AlreadyExists(path.clone());
        assert!(exists.message().contains("already exists"));

        let updated = InitResult::Updated(path);
        assert!(updated.message().contains("Updated"));
    }
}
