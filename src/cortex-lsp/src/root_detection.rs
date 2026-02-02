//! LSP root detection for finding project roots.
//!
//! This module provides per-server root detection by walking up
//! the directory tree and looking for project markers.

use crate::markers::{find_markers, should_exclude_dir, ProjectMarker};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use tracing::debug;

/// Configuration for detecting project roots for a specific server.
#[derive(Debug, Clone)]
pub struct RootDetectionConfig {
    /// Server ID this config is for.
    pub server_id: String,
    /// Markers that indicate a project root (in priority order).
    pub root_markers: Vec<ProjectMarker>,
    /// Markers that should exclude this directory (e.g., deno.json excludes TypeScript).
    pub exclude_markers: Vec<ProjectMarker>,
    /// Marker that indicates a workspace root.
    pub workspace_marker: Option<ProjectMarker>,
}

impl RootDetectionConfig {
    pub fn new(server_id: impl Into<String>) -> Self {
        Self {
            server_id: server_id.into(),
            root_markers: Vec::new(),
            exclude_markers: Vec::new(),
            workspace_marker: None,
        }
    }

    pub fn with_markers(mut self, markers: Vec<ProjectMarker>) -> Self {
        self.root_markers = markers;
        self
    }

    pub fn with_exclusions(mut self, markers: Vec<ProjectMarker>) -> Self {
        self.exclude_markers = markers;
        self
    }

    pub fn with_workspace_marker(mut self, marker: ProjectMarker) -> Self {
        self.workspace_marker = Some(marker);
        self
    }
}

/// Get the default root detection config for each builtin server.
pub fn get_server_root_config(server_id: &str) -> RootDetectionConfig {
    match server_id {
        // TypeScript/JavaScript
        "typescript" => RootDetectionConfig::new("typescript")
            .with_markers(vec![
                ProjectMarker::TsConfig,
                ProjectMarker::JsConfig,
                ProjectMarker::PackageJson,
                ProjectMarker::GitDir,
            ])
            .with_exclusions(vec![ProjectMarker::DenoJson]),

        // Biome
        "biome" => RootDetectionConfig::new("biome").with_markers(vec![
            ProjectMarker::BiomeJson,
            ProjectMarker::PackageJson,
            ProjectMarker::GitDir,
        ]),

        // ESLint
        "eslint" => RootDetectionConfig::new("eslint").with_markers(vec![
            ProjectMarker::EslintConfig,
            ProjectMarker::PackageJson,
            ProjectMarker::GitDir,
        ]),

        // Rust
        "rust" => RootDetectionConfig::new("rust")
            .with_markers(vec![ProjectMarker::CargoToml, ProjectMarker::GitDir])
            .with_workspace_marker(ProjectMarker::CargoToml),

        // Go
        "go" => RootDetectionConfig::new("go")
            .with_markers(vec![
                ProjectMarker::GoMod,
                ProjectMarker::GoWork,
                ProjectMarker::GitDir,
            ])
            .with_workspace_marker(ProjectMarker::GoWork),

        // Python
        "python" => RootDetectionConfig::new("python").with_markers(vec![
            ProjectMarker::PyProjectToml,
            ProjectMarker::SetupPy,
            ProjectMarker::SetupCfg,
            ProjectMarker::RequirementsTxt,
            ProjectMarker::Pipfile,
            ProjectMarker::GitDir,
        ]),

        // C/C++ (clangd)
        "clangd" => RootDetectionConfig::new("clangd").with_markers(vec![
            ProjectMarker::CompileCommands,
            ProjectMarker::CMakeLists,
            ProjectMarker::Makefile,
            ProjectMarker::CppProperties,
            ProjectMarker::GitDir,
        ]),

        // Java
        "java" => RootDetectionConfig::new("java")
            .with_markers(vec![
                ProjectMarker::PomXml,
                ProjectMarker::BuildGradle,
                ProjectMarker::BuildGradleKts,
                ProjectMarker::SettingsGradle,
                ProjectMarker::GitDir,
            ])
            .with_workspace_marker(ProjectMarker::SettingsGradle),

        // Lua
        "lua" => RootDetectionConfig::new("lua").with_markers(vec![
            ProjectMarker::Custom(".luarc.json".into()),
            ProjectMarker::Custom(".luacheckrc".into()),
            ProjectMarker::GitDir,
        ]),

        // YAML
        "yaml" => RootDetectionConfig::new("yaml").with_markers(vec![ProjectMarker::GitDir]),

        // JSON
        "json" => RootDetectionConfig::new("json")
            .with_markers(vec![ProjectMarker::PackageJson, ProjectMarker::GitDir]),

        // HTML
        "html" => RootDetectionConfig::new("html")
            .with_markers(vec![ProjectMarker::PackageJson, ProjectMarker::GitDir]),

        // CSS
        "css" => RootDetectionConfig::new("css")
            .with_markers(vec![ProjectMarker::PackageJson, ProjectMarker::GitDir]),

        // Bash
        "bash" => RootDetectionConfig::new("bash").with_markers(vec![ProjectMarker::GitDir]),

        // Docker
        "docker" => RootDetectionConfig::new("docker").with_markers(vec![
            ProjectMarker::DockerCompose,
            ProjectMarker::Dockerfile,
            ProjectMarker::GitDir,
        ]),

        // Terraform
        "terraform" => RootDetectionConfig::new("terraform").with_markers(vec![
            ProjectMarker::TerraformMain,
            ProjectMarker::TerraformTfvars,
            ProjectMarker::GitDir,
        ]),

        // Zig
        "zig" => RootDetectionConfig::new("zig").with_markers(vec![
            ProjectMarker::BuildZig,
            ProjectMarker::ZigZon,
            ProjectMarker::GitDir,
        ]),

        // Elixir
        "elixir" => RootDetectionConfig::new("elixir")
            .with_markers(vec![ProjectMarker::MixExs, ProjectMarker::GitDir]),

        // Vue
        "vue" => RootDetectionConfig::new("vue").with_markers(vec![
            ProjectMarker::ViteConfig,
            ProjectMarker::NuxtConfig,
            ProjectMarker::PackageJson,
            ProjectMarker::GitDir,
        ]),

        // Svelte
        "svelte" => RootDetectionConfig::new("svelte").with_markers(vec![
            ProjectMarker::SvelteConfig,
            ProjectMarker::ViteConfig,
            ProjectMarker::PackageJson,
            ProjectMarker::GitDir,
        ]),

        // Astro
        "astro" => RootDetectionConfig::new("astro").with_markers(vec![
            ProjectMarker::AstroConfig,
            ProjectMarker::PackageJson,
            ProjectMarker::GitDir,
        ]),

        // Tailwind CSS
        "tailwindcss" => RootDetectionConfig::new("tailwindcss").with_markers(vec![
            ProjectMarker::Custom("tailwind.config.js".into()),
            ProjectMarker::Custom("tailwind.config.ts".into()),
            ProjectMarker::Custom("tailwind.config.cjs".into()),
            ProjectMarker::PackageJson,
            ProjectMarker::GitDir,
        ]),

        // GraphQL
        "graphql" => RootDetectionConfig::new("graphql").with_markers(vec![
            ProjectMarker::Custom(".graphqlrc".into()),
            ProjectMarker::Custom("graphql.config.js".into()),
            ProjectMarker::Custom("graphql.config.ts".into()),
            ProjectMarker::PackageJson,
            ProjectMarker::GitDir,
        ]),

        // Prisma
        "prisma" => RootDetectionConfig::new("prisma").with_markers(vec![
            ProjectMarker::PrismaSchema,
            ProjectMarker::PackageJson,
            ProjectMarker::GitDir,
        ]),

        // SQL
        "sql" => RootDetectionConfig::new("sql").with_markers(vec![ProjectMarker::GitDir]),

        // Markdown
        "markdown" => {
            RootDetectionConfig::new("markdown").with_markers(vec![ProjectMarker::GitDir])
        }

        // TOML
        "toml" => RootDetectionConfig::new("toml").with_markers(vec![
            ProjectMarker::CargoToml,
            ProjectMarker::PyProjectToml,
            ProjectMarker::GitDir,
        ]),

        // Default fallback
        _ => RootDetectionConfig::new(server_id).with_markers(vec![ProjectMarker::GitDir]),
    }
}

/// Result of root detection.
#[derive(Debug, Clone)]
pub struct DetectedRoot {
    /// The detected root path.
    pub path: PathBuf,
    /// The marker that identified this root.
    pub marker: ProjectMarker,
    /// Whether this is a workspace root.
    pub is_workspace: bool,
    /// Child roots (for monorepo support).
    pub child_roots: Vec<PathBuf>,
}

impl DetectedRoot {
    pub fn new(path: PathBuf, marker: ProjectMarker) -> Self {
        Self {
            path,
            marker,
            is_workspace: false,
            child_roots: Vec::new(),
        }
    }
}

/// Helper for finding the nearest project root by walking up the directory tree.
pub struct NearestRoot {
    /// Cache of detected roots per server.
    cache: RwLock<HashMap<(String, PathBuf), Option<DetectedRoot>>>,
}

impl Default for NearestRoot {
    fn default() -> Self {
        Self::new()
    }
}

impl NearestRoot {
    pub fn new() -> Self {
        Self {
            cache: RwLock::new(HashMap::new()),
        }
    }

    /// Clear the cache.
    pub fn clear_cache(&self) {
        if let Ok(mut cache) = self.cache.write() {
            cache.clear();
        }
    }

    /// Find the nearest root for a file, given a server's detection config.
    pub fn find_root(
        &self,
        file_path: &Path,
        config: &RootDetectionConfig,
    ) -> Option<DetectedRoot> {
        // Try to canonicalize, but fall back to the original path if it fails
        // (e.g., the file doesn't exist yet)
        let start_dir = if file_path.is_file() {
            file_path.canonicalize().ok()?.parent()?.to_path_buf()
        } else if file_path.is_dir() {
            file_path.canonicalize().ok()?
        } else {
            // File doesn't exist - use parent directory if possible
            let parent = file_path.parent()?;
            if parent.exists() {
                parent.canonicalize().ok()?
            } else {
                // Try to find an existing ancestor
                let mut current = file_path.to_path_buf();
                loop {
                    if current.exists() {
                        break current.canonicalize().ok()?;
                    }
                    current = current.parent()?.to_path_buf();
                }
            }
        };

        // Check cache
        let cache_key = (config.server_id.clone(), start_dir.clone());
        if let Ok(cache) = self.cache.read() {
            if let Some(cached) = cache.get(&cache_key) {
                return cached.clone();
            }
        }

        // Walk up the directory tree
        let result = self.walk_up(&start_dir, config);

        // Cache the result
        if let Ok(mut cache) = self.cache.write() {
            cache.insert(cache_key, result.clone());
        }

        result
    }

    fn walk_up(&self, start: &Path, config: &RootDetectionConfig) -> Option<DetectedRoot> {
        let mut current = start.to_path_buf();
        let mut workspace_root: Option<DetectedRoot> = None;
        let mut best_root: Option<DetectedRoot> = None;

        loop {
            // Check for exclusion markers first
            if !config.exclude_markers.is_empty() {
                let exclusions = find_markers(&current, &config.exclude_markers);
                if !exclusions.is_empty() {
                    debug!(
                        "Excluded directory {} for server {} due to {:?}",
                        current.display(),
                        config.server_id,
                        exclusions[0].marker
                    );
                    // Skip this directory but continue walking up
                    if let Some(parent) = current.parent() {
                        current = parent.to_path_buf();
                        continue;
                    } else {
                        break;
                    }
                }
            }

            // Find markers in this directory
            let markers = find_markers(&current, &config.root_markers);

            for marker_match in markers {
                // Check if this is a workspace marker
                if marker_match.is_workspace {
                    workspace_root = Some(DetectedRoot {
                        path: current.clone(),
                        marker: marker_match.marker.clone(),
                        is_workspace: true,
                        child_roots: Vec::new(),
                    });
                } else if best_root.is_none() {
                    // First (best priority) non-workspace root
                    best_root = Some(DetectedRoot::new(
                        current.clone(),
                        marker_match.marker.clone(),
                    ));
                }
            }

            // Move to parent
            if let Some(parent) = current.parent() {
                // Don't go above filesystem root
                if parent == current {
                    break;
                }
                current = parent.to_path_buf();
            } else {
                break;
            }
        }

        // Prefer workspace root if found, otherwise use best root
        if let Some(mut ws) = workspace_root {
            if let Some(child) = best_root {
                if ws.path != child.path {
                    ws.child_roots.push(child.path);
                }
            }
            Some(ws)
        } else {
            best_root
        }
    }

    /// Find all roots in a directory (for monorepo scanning).
    pub fn find_all_roots(&self, dir: &Path, config: &RootDetectionConfig) -> Vec<DetectedRoot> {
        let mut roots = Vec::new();
        self.scan_directory(dir, config, &mut roots, 0);
        roots
    }

    fn scan_directory(
        &self,
        dir: &Path,
        config: &RootDetectionConfig,
        roots: &mut Vec<DetectedRoot>,
        depth: usize,
    ) {
        // Limit recursion depth
        if depth > 10 {
            return;
        }

        // Check for markers in this directory
        let markers = find_markers(dir, &config.root_markers);

        // Check for exclusions
        if !config.exclude_markers.is_empty() {
            let exclusions = find_markers(dir, &config.exclude_markers);
            if !exclusions.is_empty() {
                return;
            }
        }

        if let Some(best) = markers.first() {
            roots.push(DetectedRoot {
                path: dir.to_path_buf(),
                marker: best.marker.clone(),
                is_workspace: best.is_workspace,
                child_roots: Vec::new(),
            });

            // If it's a workspace, scan children
            if best.is_workspace {
                self.scan_children(dir, config, roots, depth);
            }
        } else {
            // No root marker here, scan children
            self.scan_children(dir, config, roots, depth);
        }
    }

    fn scan_children(
        &self,
        dir: &Path,
        config: &RootDetectionConfig,
        roots: &mut Vec<DetectedRoot>,
        depth: usize,
    ) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                if let Ok(file_type) = entry.file_type() {
                    if file_type.is_dir() {
                        let name = entry.file_name();
                        let name_str = name.to_string_lossy();

                        if !should_exclude_dir(&name_str) {
                            self.scan_directory(&entry.path(), config, roots, depth + 1);
                        }
                    }
                }
            }
        }
    }
}

lazy_static::lazy_static! {
    /// Global root detector instance.
    pub static ref ROOT_DETECTOR: NearestRoot = NearestRoot::new();
}

/// Convenience function to detect root for a file and server.
pub fn detect_root(file_path: &Path, server_id: &str) -> Option<DetectedRoot> {
    let config = get_server_root_config(server_id);
    ROOT_DETECTOR.find_root(file_path, &config)
}

/// Convenience function to detect root with custom config.
pub fn detect_root_with_config(
    file_path: &Path,
    config: &RootDetectionConfig,
) -> Option<DetectedRoot> {
    ROOT_DETECTOR.find_root(file_path, config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_nearest_root_simple() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();

        // Create package.json at root
        fs::write(root.join("package.json"), "{}").unwrap();

        // Create nested directory
        fs::create_dir_all(root.join("src/components")).unwrap();

        let detector = NearestRoot::new();
        let config = get_server_root_config("typescript");

        let detected = detector.find_root(&root.join("src/components/test.ts"), &config);
        assert!(detected.is_some());
        assert_eq!(detected.unwrap().path, root);
    }

    #[test]
    fn test_exclusion_markers() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();

        // Create deno.json (should exclude typescript server)
        fs::write(root.join("deno.json"), "{}").unwrap();
        fs::write(root.join("package.json"), "{}").unwrap();

        let detector = NearestRoot::new();
        let config = get_server_root_config("typescript");

        let detected = detector.find_root(&root.join("test.ts"), &config);
        // Should not find root because deno.json excludes typescript
        assert!(detected.is_none());
    }

    #[test]
    fn test_cargo_workspace_detection() {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();

        // Create workspace Cargo.toml
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]",
        )
        .unwrap();

        // Create member crate
        fs::create_dir_all(root.join("crates/my-crate/src")).unwrap();
        fs::write(
            root.join("crates/my-crate/Cargo.toml"),
            "[package]\nname = \"my-crate\"",
        )
        .unwrap();

        let detector = NearestRoot::new();
        let config = get_server_root_config("rust");

        let detected = detector.find_root(&root.join("crates/my-crate/src/lib.rs"), &config);
        assert!(detected.is_some());
        let detected = detected.unwrap();
        assert!(detected.is_workspace);
        assert_eq!(detected.path, root);
    }
}
