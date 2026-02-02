//! Templates for built-in commands.
//!
//! This module contains default templates for generating configuration files.

/// Default template for AGENTS.md file.
///
/// This template provides a starting point for project-specific AI agent instructions.
/// Placeholders like `{project_name}`, `{project_type}`, etc. can be replaced
/// with actual values detected from the project.
pub const AGENTS_MD_TEMPLATE: &str = r#"# AGENTS.md

## Project Overview

{project_description}

## Architecture

{architecture_description}

## Key Files

{key_files}

## Development Guidelines

### Code Style
{code_style_guidelines}

### Testing
{testing_guidelines}

### Git Workflow
- Use conventional commits (feat:, fix:, docs:, refactor:, test:, chore:)
- Create feature branches for new work
- Keep commits focused and atomic

## Common Tasks

### Building
```bash
{build_command}
```

### Testing
```bash
{test_command}
```

### Running
```bash
{run_command}
```

## Notes for AI Agents

- This is a {project_type} project
- Focus on {focus_areas}
- Avoid {things_to_avoid}
- When making changes, prefer small, focused modifications
- Ask clarifying questions when requirements are ambiguous
- Explain reasoning for significant architectural decisions
"#;

/// Minimal template for AGENTS.md when no project type is detected.
pub const AGENTS_MD_MINIMAL_TEMPLATE: &str = r#"# AGENTS.md

## Project Overview

<!-- Describe your project here -->

## Coding Conventions

- Follow existing code style
- Use meaningful variable and function names
- Add comments for complex logic

## File Structure

<!-- Describe important directories -->

## Testing

- Run tests before committing
- Add tests for new features

## Build & Run

<!-- Add build/run instructions -->

## Notes for AI Agent

- Prefer small, focused changes
- Ask clarifying questions when unsure
- Explain reasoning for significant changes
"#;

/// Template for Rust projects.
pub const RUST_PROJECT_DEFAULTS: ProjectDefaults = ProjectDefaults {
    project_type: "Rust",
    build_command: "cargo build",
    test_command: "cargo test",
    run_command: "cargo run",
    code_style: &[
        "Follow Rust formatting guidelines (rustfmt)",
        "Use `cargo clippy` for linting",
        "Prefer `Result` and `Option` over panics",
        "Document public APIs with doc comments",
        "When adding new slash commands, register them in BOTH places: cortex-tui/src/commands/registry.rs (for autocomplete) AND cortex-tui/src/commands/executor.rs (for execution)",
    ],
    testing: &[
        "Write unit tests in the same file using `#[cfg(test)]`",
        "Use `#[test]` attribute for test functions",
        "Run `cargo test` before committing",
    ],
    focus_areas: "type safety, error handling, performance",
    things_to_avoid: "unwrap() in production code, mutable global state",
    key_files: &[
        ("Cargo.toml", "Project manifest and dependencies"),
        ("src/main.rs", "Application entry point"),
        ("src/lib.rs", "Library root module"),
    ],
};

/// Template for Node.js projects.
pub const NODE_PROJECT_DEFAULTS: ProjectDefaults = ProjectDefaults {
    project_type: "Node.js/TypeScript",
    build_command: "npm run build",
    test_command: "npm test",
    run_command: "npm start",
    code_style: &[
        "Follow ESLint/Prettier configuration",
        "Use TypeScript strict mode when available",
        "Prefer async/await over callbacks",
        "Use meaningful variable and function names",
    ],
    testing: &[
        "Write tests using Jest or the configured test framework",
        "Aim for good test coverage",
        "Run `npm test` before committing",
    ],
    focus_areas: "type safety (if TS), error handling, async patterns",
    things_to_avoid: "any type overuse, callback hell, synchronous I/O",
    key_files: &[
        ("package.json", "Project manifest and dependencies"),
        ("tsconfig.json", "TypeScript configuration"),
        ("src/index.ts", "Application entry point"),
    ],
};

/// Template for Python projects.
pub const PYTHON_PROJECT_DEFAULTS: ProjectDefaults = ProjectDefaults {
    project_type: "Python",
    build_command: "pip install -e .",
    test_command: "pytest",
    run_command: "python -m {module_name}",
    code_style: &[
        "Follow PEP 8 style guidelines",
        "Use type hints for function signatures",
        "Use Black or similar formatter",
        "Document functions with docstrings",
    ],
    testing: &[
        "Write tests using pytest",
        "Use fixtures for test setup",
        "Run `pytest` before committing",
    ],
    focus_areas: "readability, type hints, error handling",
    things_to_avoid: "bare except clauses, mutable default arguments",
    key_files: &[
        ("pyproject.toml", "Project configuration"),
        ("setup.py", "Package setup (legacy)"),
        ("requirements.txt", "Dependencies"),
    ],
};

/// Template for Go projects.
pub const GO_PROJECT_DEFAULTS: ProjectDefaults = ProjectDefaults {
    project_type: "Go",
    build_command: "go build",
    test_command: "go test ./...",
    run_command: "go run .",
    code_style: &[
        "Follow Go formatting guidelines (gofmt)",
        "Use golint and go vet",
        "Keep functions small and focused",
        "Handle all errors explicitly",
    ],
    testing: &[
        "Write tests in *_test.go files",
        "Use table-driven tests",
        "Run `go test ./...` before committing",
    ],
    focus_areas: "simplicity, error handling, concurrency safety",
    things_to_avoid: "ignoring errors, global state, overly complex abstractions",
    key_files: &[
        ("go.mod", "Module definition and dependencies"),
        ("main.go", "Application entry point"),
    ],
};

/// Default values for a project type.
#[derive(Debug, Clone, Copy)]
pub struct ProjectDefaults {
    /// The project type name.
    pub project_type: &'static str,
    /// Default build command.
    pub build_command: &'static str,
    /// Default test command.
    pub test_command: &'static str,
    /// Default run command.
    pub run_command: &'static str,
    /// Code style guidelines.
    pub code_style: &'static [&'static str],
    /// Testing guidelines.
    pub testing: &'static [&'static str],
    /// Focus areas for AI agents.
    pub focus_areas: &'static str,
    /// Things to avoid.
    pub things_to_avoid: &'static str,
    /// Key files with descriptions.
    pub key_files: &'static [(&'static str, &'static str)],
}

impl ProjectDefaults {
    /// Format code style guidelines as markdown list.
    pub fn format_code_style(&self) -> String {
        self.code_style
            .iter()
            .map(|s| format!("- {s}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Format testing guidelines as markdown list.
    pub fn format_testing(&self) -> String {
        self.testing
            .iter()
            .map(|s| format!("- {s}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Format key files as markdown list.
    pub fn format_key_files(&self) -> String {
        self.key_files
            .iter()
            .map(|(file, desc)| format!("- `{file}` - {desc}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rust_defaults() {
        assert_eq!(RUST_PROJECT_DEFAULTS.project_type, "Rust");
        assert_eq!(RUST_PROJECT_DEFAULTS.build_command, "cargo build");
        assert!(!RUST_PROJECT_DEFAULTS.code_style.is_empty());
    }

    #[test]
    fn test_format_code_style() {
        let style = RUST_PROJECT_DEFAULTS.format_code_style();
        assert!(style.contains("- Follow Rust formatting guidelines"));
    }

    #[test]
    fn test_format_key_files() {
        let files = RUST_PROJECT_DEFAULTS.format_key_files();
        assert!(files.contains("`Cargo.toml`"));
    }

    #[test]
    fn test_minimal_template_not_empty() {
        assert!(!AGENTS_MD_MINIMAL_TEMPLATE.is_empty());
        assert!(AGENTS_MD_MINIMAL_TEMPLATE.contains("# AGENTS.md"));
    }
}
