//! Project marker definitions for LSP root detection.
//!
//! This module defines project markers used to identify project roots
//! for different language servers.

#![allow(
    clippy::manual_contains,
    clippy::if_same_then_else,
    clippy::collapsible_else_if
)]

use std::path::{Path, PathBuf};

/// Project marker types for different languages/servers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProjectMarker {
    // JavaScript/TypeScript ecosystem
    PackageJson,
    TsConfig,
    JsConfig,
    DenoJson,
    BunLockb,
    NodeModules,

    // Rust
    CargoToml,
    CargoLock,

    // Go
    GoMod,
    GoWork,
    GoSum,

    // Python
    PyProjectToml,
    SetupPy,
    SetupCfg,
    RequirementsTxt,
    Pipfile,
    PoetryLock,
    PyvenvCfg,

    // Java/JVM
    PomXml,
    BuildGradle,
    BuildGradleKts,
    SettingsGradle,

    // C/C++
    CMakeLists,
    Makefile,
    CompileCommands,
    CppProperties,

    // Zig
    BuildZig,
    ZigZon,

    // Elixir
    MixExs,

    // Terraform
    TerraformTfvars,
    TerraformMain,

    // Docker
    Dockerfile,
    DockerCompose,

    // Generic
    GitDir,
    EditorConfig,

    // Web frameworks
    NextConfig,
    NuxtConfig,
    ViteConfig,
    SvelteConfig,
    AstroConfig,

    // Linters/Formatters
    BiomeJson,
    EslintConfig,
    PrettierConfig,

    // Database
    PrismaSchema,

    // Custom marker by filename
    Custom(String),
}

impl ProjectMarker {
    /// Get the filename(s) that this marker represents.
    pub fn filenames(&self) -> Vec<&str> {
        match self {
            // JavaScript/TypeScript
            ProjectMarker::PackageJson => vec!["package.json"],
            ProjectMarker::TsConfig => vec!["tsconfig.json", "tsconfig.base.json"],
            ProjectMarker::JsConfig => vec!["jsconfig.json"],
            ProjectMarker::DenoJson => vec!["deno.json", "deno.jsonc"],
            ProjectMarker::BunLockb => vec!["bun.lockb"],
            ProjectMarker::NodeModules => vec!["node_modules"],

            // Rust
            ProjectMarker::CargoToml => vec!["Cargo.toml"],
            ProjectMarker::CargoLock => vec!["Cargo.lock"],

            // Go
            ProjectMarker::GoMod => vec!["go.mod"],
            ProjectMarker::GoWork => vec!["go.work"],
            ProjectMarker::GoSum => vec!["go.sum"],

            // Python
            ProjectMarker::PyProjectToml => vec!["pyproject.toml"],
            ProjectMarker::SetupPy => vec!["setup.py"],
            ProjectMarker::SetupCfg => vec!["setup.cfg"],
            ProjectMarker::RequirementsTxt => vec!["requirements.txt"],
            ProjectMarker::Pipfile => vec!["Pipfile"],
            ProjectMarker::PoetryLock => vec!["poetry.lock"],
            ProjectMarker::PyvenvCfg => vec!["pyvenv.cfg"],

            // Java
            ProjectMarker::PomXml => vec!["pom.xml"],
            ProjectMarker::BuildGradle => vec!["build.gradle"],
            ProjectMarker::BuildGradleKts => vec!["build.gradle.kts"],
            ProjectMarker::SettingsGradle => vec!["settings.gradle", "settings.gradle.kts"],

            // C/C++
            ProjectMarker::CMakeLists => vec!["CMakeLists.txt"],
            ProjectMarker::Makefile => vec!["Makefile", "makefile", "GNUmakefile"],
            ProjectMarker::CompileCommands => vec!["compile_commands.json"],
            ProjectMarker::CppProperties => vec!["c_cpp_properties.json"],

            // Zig
            ProjectMarker::BuildZig => vec!["build.zig"],
            ProjectMarker::ZigZon => vec!["build.zig.zon"],

            // Elixir
            ProjectMarker::MixExs => vec!["mix.exs"],

            // Terraform
            ProjectMarker::TerraformTfvars => vec!["terraform.tfvars"],
            ProjectMarker::TerraformMain => vec!["main.tf"],

            // Docker
            ProjectMarker::Dockerfile => vec!["Dockerfile", "dockerfile"],
            ProjectMarker::DockerCompose => vec![
                "docker-compose.yml",
                "docker-compose.yaml",
                "compose.yml",
                "compose.yaml",
            ],

            // Generic
            ProjectMarker::GitDir => vec![".git"],
            ProjectMarker::EditorConfig => vec![".editorconfig"],

            // Web frameworks
            ProjectMarker::NextConfig => {
                vec!["next.config.js", "next.config.mjs", "next.config.ts"]
            }
            ProjectMarker::NuxtConfig => vec!["nuxt.config.js", "nuxt.config.ts"],
            ProjectMarker::ViteConfig => {
                vec!["vite.config.js", "vite.config.ts", "vite.config.mjs"]
            }
            ProjectMarker::SvelteConfig => vec!["svelte.config.js"],
            ProjectMarker::AstroConfig => vec!["astro.config.mjs", "astro.config.ts"],

            // Linters
            ProjectMarker::BiomeJson => vec!["biome.json", "biome.jsonc"],
            ProjectMarker::EslintConfig => vec![
                ".eslintrc",
                ".eslintrc.js",
                ".eslintrc.json",
                ".eslintrc.yml",
                "eslint.config.js",
                "eslint.config.mjs",
            ],
            ProjectMarker::PrettierConfig => vec![
                ".prettierrc",
                ".prettierrc.js",
                ".prettierrc.json",
                "prettier.config.js",
            ],

            // Database
            ProjectMarker::PrismaSchema => vec!["schema.prisma"],

            // Custom
            ProjectMarker::Custom(name) => vec![name.as_str()],
        }
    }

    /// Check if a file/directory matches this marker.
    pub fn matches(&self, name: &str) -> bool {
        self.filenames().iter().any(|f| *f == name)
    }

    /// Get the priority of this marker (higher = more specific/preferred).
    pub fn priority(&self) -> u8 {
        match self {
            // Workspace markers have highest priority
            ProjectMarker::GoWork => 100,
            ProjectMarker::CargoToml => 95, // Could be workspace
            ProjectMarker::SettingsGradle => 95,

            // Primary project files
            ProjectMarker::PackageJson => 90,
            ProjectMarker::GoMod => 90,
            ProjectMarker::PyProjectToml => 90,
            ProjectMarker::PomXml => 90,
            ProjectMarker::BuildGradle | ProjectMarker::BuildGradleKts => 85,
            ProjectMarker::CMakeLists => 85,
            ProjectMarker::BuildZig => 85,
            ProjectMarker::MixExs => 85,

            // Config files
            ProjectMarker::TsConfig | ProjectMarker::JsConfig => 80,
            ProjectMarker::DenoJson => 80,

            // Secondary markers
            ProjectMarker::SetupPy | ProjectMarker::SetupCfg => 70,
            ProjectMarker::RequirementsTxt | ProjectMarker::Pipfile => 60,
            ProjectMarker::Makefile => 60,
            ProjectMarker::CompileCommands => 75,

            // Framework configs
            ProjectMarker::NextConfig | ProjectMarker::NuxtConfig => 70,
            ProjectMarker::ViteConfig
            | ProjectMarker::SvelteConfig
            | ProjectMarker::AstroConfig => 70,

            // Lock files (less priority, they're alongside main files)
            ProjectMarker::CargoLock | ProjectMarker::GoSum | ProjectMarker::PoetryLock => 40,
            ProjectMarker::BunLockb => 40,

            // Tooling
            ProjectMarker::BiomeJson
            | ProjectMarker::EslintConfig
            | ProjectMarker::PrettierConfig => 50,
            ProjectMarker::PrismaSchema => 60,

            // Generic/fallback
            ProjectMarker::GitDir => 10,
            ProjectMarker::EditorConfig => 5,
            ProjectMarker::NodeModules => 30,

            // Docker
            ProjectMarker::Dockerfile => 50,
            ProjectMarker::DockerCompose => 55,

            // Terraform
            ProjectMarker::TerraformMain | ProjectMarker::TerraformTfvars => 70,

            // Environment
            ProjectMarker::CppProperties => 60,
            ProjectMarker::ZigZon => 80,
            ProjectMarker::PyvenvCfg => 30,

            ProjectMarker::Custom(_) => 50,
        }
    }
}

/// Result of finding markers in a directory.
#[derive(Debug, Clone)]
pub struct MarkerMatch {
    /// The path where the marker was found.
    pub path: PathBuf,
    /// The marker that was found.
    pub marker: ProjectMarker,
    /// Whether this is a workspace marker (e.g., Cargo workspace, Go workspace).
    pub is_workspace: bool,
}

impl MarkerMatch {
    pub fn new(path: PathBuf, marker: ProjectMarker) -> Self {
        Self {
            path,
            marker,
            is_workspace: false,
        }
    }

    pub fn with_workspace(mut self, is_workspace: bool) -> Self {
        self.is_workspace = is_workspace;
        self
    }
}

/// Find markers in a directory for a specific server.
pub fn find_markers(dir: &Path, markers: &[ProjectMarker]) -> Vec<MarkerMatch> {
    let mut results = Vec::new();

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            for marker in markers {
                if marker.matches(&name_str) {
                    let mut marker_match = MarkerMatch::new(entry.path(), marker.clone());

                    // Check if it's a workspace marker
                    if matches!(marker, ProjectMarker::CargoToml) {
                        if let Ok(content) = std::fs::read_to_string(entry.path()) {
                            if content.contains("[workspace]") {
                                marker_match = marker_match.with_workspace(true);
                            }
                        }
                    } else if matches!(marker, ProjectMarker::GoWork) {
                        marker_match = marker_match.with_workspace(true);
                    } else if matches!(marker, ProjectMarker::SettingsGradle) {
                        marker_match = marker_match.with_workspace(true);
                    } else if matches!(marker, ProjectMarker::PackageJson) {
                        if let Ok(content) = std::fs::read_to_string(entry.path()) {
                            if content.contains("\"workspaces\"") {
                                marker_match = marker_match.with_workspace(true);
                            }
                        }
                    }

                    results.push(marker_match);
                }
            }
        }
    }

    // Sort by priority (highest first)
    results.sort_by(|a, b| b.marker.priority().cmp(&a.marker.priority()));

    results
}

/// Check if a directory should be excluded from root detection.
pub fn should_exclude_dir(name: &str) -> bool {
    matches!(
        name,
        "node_modules"
            | ".git"
            | "target"
            | "build"
            | "dist"
            | ".next"
            | ".nuxt"
            | "__pycache__"
            | ".pytest_cache"
            | "venv"
            | ".venv"
            | "env"
            | ".env"
            | "vendor"
            | ".cargo"
            | "zig-cache"
            | "_build"
            | "deps"
            | ".terraform"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_marker_filenames() {
        assert!(ProjectMarker::PackageJson
            .filenames()
            .contains(&"package.json"));
        assert!(ProjectMarker::CargoToml.filenames().contains(&"Cargo.toml"));
    }

    #[test]
    fn test_marker_matches() {
        assert!(ProjectMarker::PackageJson.matches("package.json"));
        assert!(!ProjectMarker::PackageJson.matches("package-lock.json"));
    }

    #[test]
    fn test_marker_priority() {
        assert!(ProjectMarker::PackageJson.priority() > ProjectMarker::GitDir.priority());
        assert!(ProjectMarker::GoWork.priority() > ProjectMarker::GoMod.priority());
    }
}
