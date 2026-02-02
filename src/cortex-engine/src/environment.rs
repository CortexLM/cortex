//! Environment context and system information.
//!
//! Provides comprehensive environment detection, system information gathering,
//! and context building for agent operations.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Environment context containing system and project information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentContext {
    /// Operating system info.
    pub os: OsInfo,
    /// Shell information.
    pub shell: ShellInfo,
    /// Current working directory.
    pub cwd: PathBuf,
    /// User information.
    pub user: UserInfo,
    /// Project context.
    pub project: Option<ProjectContext>,
    /// Git context.
    pub git: Option<GitContext>,
    /// Runtime environment.
    pub runtime: RuntimeInfo,
    /// Installed tools.
    pub tools: Vec<InstalledTool>,
    /// Environment variables (filtered).
    pub env_vars: HashMap<String, String>,
    /// System resources.
    pub resources: SystemResources,
}

impl EnvironmentContext {
    /// Gather environment context.
    pub async fn gather() -> Result<Self> {
        let cwd = std::env::current_dir().unwrap_or_default();

        Ok(Self {
            os: OsInfo::detect(),
            shell: ShellInfo::detect(),
            cwd: cwd.clone(),
            user: UserInfo::detect(),
            project: ProjectContext::detect(&cwd).await,
            git: GitContext::detect(&cwd).await,
            runtime: RuntimeInfo::detect(),
            tools: detect_tools().await,
            env_vars: gather_safe_env_vars(),
            resources: SystemResources::detect(),
        })
    }

    /// Get a summary for the system prompt.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();

        parts.push(format!("OS: {} {}", self.os.name, self.os.version));
        parts.push(format!("Shell: {}", self.shell.name));
        parts.push(format!("CWD: {}", self.cwd.display()));

        if let Some(ref project) = self.project {
            parts.push(format!(
                "Project: {} ({})",
                project.name, project.project_type
            ));
        }

        if let Some(ref git) = self.git {
            parts.push(format!("Git branch: {}", git.branch));
        }

        parts.join("\n")
    }

    /// Check if a tool is available.
    pub fn has_tool(&self, name: &str) -> bool {
        self.tools.iter().any(|t| t.name == name)
    }

    /// Get tool version.
    pub fn tool_version(&self, name: &str) -> Option<&str> {
        self.tools
            .iter()
            .find(|t| t.name == name)
            .and_then(|t| t.version.as_deref())
    }
}

/// Operating system information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsInfo {
    /// OS name (Linux, macOS, Windows).
    pub name: String,
    /// OS version.
    pub version: String,
    /// Architecture.
    pub arch: String,
    /// Kernel version.
    pub kernel: Option<String>,
    /// Distribution (for Linux).
    pub distro: Option<String>,
}

impl OsInfo {
    /// Detect OS information.
    pub fn detect() -> Self {
        let name = std::env::consts::OS.to_string();
        let arch = std::env::consts::ARCH.to_string();

        let (version, kernel, distro) = Self::detect_details();

        Self {
            name,
            version,
            arch,
            kernel,
            distro,
        }
    }

    fn detect_details() -> (String, Option<String>, Option<String>) {
        #[cfg(target_os = "linux")]
        {
            let kernel = Command::new("uname")
                .arg("-r")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string());

            let distro = std::fs::read_to_string("/etc/os-release")
                .ok()
                .and_then(|content| {
                    content
                        .lines()
                        .find(|l| l.starts_with("PRETTY_NAME="))
                        .map(|l| {
                            l.trim_start_matches("PRETTY_NAME=")
                                .trim_matches('"')
                                .to_string()
                        })
                });

            let version = distro.clone().unwrap_or_else(|| "Linux".to_string());
            (version, kernel, distro)
        }

        #[cfg(target_os = "macos")]
        {
            let version = Command::new("sw_vers")
                .arg("-productVersion")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| "macOS".to_string());

            (version, None, None)
        }

        #[cfg(target_os = "windows")]
        {
            ("Windows".to_string(), None, None)
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            ("Unknown".to_string(), None, None)
        }
    }

    /// Check if running on Linux.
    pub fn is_linux(&self) -> bool {
        self.name == "linux"
    }

    /// Check if running on macOS.
    pub fn is_macos(&self) -> bool {
        self.name == "macos"
    }

    /// Check if running on Windows.
    pub fn is_windows(&self) -> bool {
        self.name == "windows"
    }
}

/// Shell information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellInfo {
    /// Shell name (bash, zsh, fish, etc).
    pub name: String,
    /// Shell path.
    pub path: PathBuf,
    /// Shell version.
    pub version: Option<String>,
    /// Is interactive.
    pub interactive: bool,
    /// Is login shell.
    pub login: bool,
}

impl ShellInfo {
    /// Detect shell information.
    pub fn detect() -> Self {
        let shell_path = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());

        let path = PathBuf::from(&shell_path);
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("sh")
            .to_string();

        let version = Self::detect_version(&name);

        Self {
            name,
            path,
            version,
            interactive: std::env::var("PS1").is_ok(),
            login: std::env::var("LOGIN_SHELL").is_ok(),
        }
    }

    fn detect_version(shell: &str) -> Option<String> {
        let output = Command::new(shell).arg("--version").output().ok()?;

        String::from_utf8(output.stdout)
            .ok()
            .and_then(|s| s.lines().next().map(std::string::ToString::to_string))
    }
}

/// User information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    /// Username.
    pub username: String,
    /// User ID.
    pub uid: Option<u32>,
    /// Home directory.
    pub home: PathBuf,
    /// Is root/admin.
    pub is_root: bool,
    /// User groups.
    pub groups: Vec<String>,
}

impl UserInfo {
    /// Detect user information.
    pub fn detect() -> Self {
        let username = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "unknown".to_string());

        let home = dirs::home_dir().unwrap_or_default();

        #[cfg(unix)]
        let uid = Some(unsafe { libc::getuid() });
        #[cfg(not(unix))]
        let uid = None;

        let is_root = uid == Some(0);

        let groups = Self::detect_groups();

        Self {
            username,
            uid,
            home,
            is_root,
            groups,
        }
    }

    fn detect_groups() -> Vec<String> {
        Command::new("groups")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| {
                s.split_whitespace()
                    .map(std::string::ToString::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Project context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectContext {
    /// Project name.
    pub name: String,
    /// Project type.
    pub project_type: ProjectType,
    /// Project root.
    pub root: PathBuf,
    /// Package manager.
    pub package_manager: Option<String>,
    /// Main language.
    pub language: Option<String>,
    /// Framework.
    pub framework: Option<String>,
    /// Dependencies count.
    pub dependencies_count: Option<usize>,
    /// Has tests.
    pub has_tests: bool,
    /// Has CI configuration.
    pub has_ci: bool,
}

impl ProjectContext {
    /// Detect project context from path.
    pub async fn detect(path: &Path) -> Option<Self> {
        // Find project root
        let root = find_project_root(path)?;

        // Detect project type
        let project_type = detect_project_type(&root);

        let name = root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("project")
            .to_string();

        let (package_manager, language, framework, deps) =
            detect_project_details(&root, &project_type);

        let has_tests = root.join("tests").exists()
            || root.join("test").exists()
            || root.join("__tests__").exists();

        let has_ci = root.join(".github/workflows").exists()
            || root.join(".gitlab-ci.yml").exists()
            || root.join(".circleci").exists();

        Some(Self {
            name,
            project_type,
            root,
            package_manager,
            language,
            framework,
            dependencies_count: deps,
            has_tests,
            has_ci,
        })
    }
}

/// Project type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectType {
    Rust,
    Node,
    Python,
    Go,
    Java,
    Ruby,
    Php,
    Dotnet,
    Unknown,
}

impl std::fmt::Display for ProjectType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rust => write!(f, "Rust"),
            Self::Node => write!(f, "Node.js"),
            Self::Python => write!(f, "Python"),
            Self::Go => write!(f, "Go"),
            Self::Java => write!(f, "Java"),
            Self::Ruby => write!(f, "Ruby"),
            Self::Php => write!(f, "PHP"),
            Self::Dotnet => write!(f, ".NET"),
            Self::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Git context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitContext {
    /// Repository root.
    pub root: PathBuf,
    /// Current branch.
    pub branch: String,
    /// Remote URL.
    pub remote_url: Option<String>,
    /// Is dirty (has uncommitted changes).
    pub is_dirty: bool,
    /// Number of commits ahead.
    pub ahead: u32,
    /// Number of commits behind.
    pub behind: u32,
    /// Last commit hash.
    pub last_commit: Option<String>,
    /// Last commit message.
    pub last_message: Option<String>,
}

impl GitContext {
    /// Detect git context.
    pub async fn detect(path: &Path) -> Option<Self> {
        // Check if in a git repo
        let output = Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .current_dir(path)
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let root = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(path)
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| PathBuf::from(s.trim()))
            .unwrap_or_else(|| path.to_path_buf());

        let branch = Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(path)
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "main".to_string());

        let remote_url = Command::new("git")
            .args(["remote", "get-url", "origin"])
            .current_dir(path)
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string());

        let is_dirty = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(path)
            .output()
            .ok()
            .map(|o| !o.stdout.is_empty())
            .unwrap_or(false);

        let last_commit = Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .current_dir(path)
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string());

        let last_message = Command::new("git")
            .args(["log", "-1", "--pretty=%s"])
            .current_dir(path)
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string());

        Some(Self {
            root,
            branch,
            remote_url,
            is_dirty,
            ahead: 0,
            behind: 0,
            last_commit,
            last_message,
        })
    }
}

/// Runtime information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeInfo {
    /// Rust version (if available).
    pub rust_version: Option<String>,
    /// Node version (if available).
    pub node_version: Option<String>,
    /// Python version (if available).
    pub python_version: Option<String>,
    /// Go version (if available).
    pub go_version: Option<String>,
}

impl RuntimeInfo {
    /// Detect runtime information.
    pub fn detect() -> Self {
        Self {
            rust_version: detect_version("rustc", &["--version"]),
            node_version: detect_version("node", &["--version"]),
            python_version: detect_version("python3", &["--version"])
                .or_else(|| detect_version("python", &["--version"])),
            go_version: detect_version("go", &["version"]),
        }
    }
}

/// Installed tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledTool {
    /// Tool name.
    pub name: String,
    /// Tool path.
    pub path: Option<PathBuf>,
    /// Version.
    pub version: Option<String>,
}

/// System resources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemResources {
    /// CPU cores.
    pub cpu_cores: usize,
    /// Total memory in bytes.
    pub total_memory: u64,
    /// Available memory in bytes.
    pub available_memory: u64,
    /// Disk space info.
    pub disk: DiskInfo,
}

impl SystemResources {
    /// Detect system resources.
    /// Respects container CPU limits when running in containerized environments.
    pub fn detect() -> Self {
        Self {
            cpu_cores: Self::detect_cpu_cores(),
            total_memory: 0, // Would use sysinfo crate
            available_memory: 0,
            disk: DiskInfo::default(),
        }
    }

    /// Detect the number of available CPU cores, respecting cgroup limits.
    /// This is important for containers/VMs where the host may report more CPUs
    /// than are actually available to the process.
    fn detect_cpu_cores() -> usize {
        // First try to detect cgroup CPU limits (Linux containers)
        if let Some(cgroup_cpus) = Self::detect_cgroup_cpu_limit() {
            return cgroup_cpus;
        }

        // Fall back to std::thread::available_parallelism which is cgroup-aware
        // on newer Rust versions
        std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or_else(|_| num_cpus::get())
    }

    /// Try to detect cgroup CPU limits.
    /// Returns None if not in a cgroup or unable to read limits.
    fn detect_cgroup_cpu_limit() -> Option<usize> {
        // Try cgroup v2 first
        if let Some(cpus) = Self::detect_cgroup_v2_cpu_limit() {
            return Some(cpus);
        }

        // Fall back to cgroup v1
        Self::detect_cgroup_v1_cpu_limit()
    }

    /// Detect cgroup v2 CPU limits.
    #[cfg(target_os = "linux")]
    fn detect_cgroup_v2_cpu_limit() -> Option<usize> {
        // cgroup v2 uses cpu.max file with format "max 100000" or "200000 100000"
        // where first number is quota and second is period
        let cpu_max = std::fs::read_to_string("/sys/fs/cgroup/cpu.max").ok()?;
        let parts: Vec<&str> = cpu_max.trim().split_whitespace().collect();

        if parts.len() >= 2 {
            let quota = parts[0];
            let period = parts[1];

            if quota == "max" {
                return None; // No limit set
            }

            let quota: f64 = quota.parse().ok()?;
            let period: f64 = period.parse().ok()?;

            if period > 0.0 {
                let cpus = (quota / period).ceil() as usize;
                return Some(cpus.max(1));
            }
        }

        None
    }

    /// Detect cgroup v1 CPU limits.
    #[cfg(target_os = "linux")]
    fn detect_cgroup_v1_cpu_limit() -> Option<usize> {
        // cgroup v1 uses cpu.cfs_quota_us and cpu.cfs_period_us
        let quota_path = "/sys/fs/cgroup/cpu/cpu.cfs_quota_us";
        let period_path = "/sys/fs/cgroup/cpu/cpu.cfs_period_us";

        let quota: i64 = std::fs::read_to_string(quota_path)
            .ok()?
            .trim()
            .parse()
            .ok()?;

        // -1 means no limit
        if quota < 0 {
            return None;
        }

        let period: i64 = std::fs::read_to_string(period_path)
            .ok()?
            .trim()
            .parse()
            .ok()?;

        if period > 0 {
            let cpus = ((quota as f64) / (period as f64)).ceil() as usize;
            return Some(cpus.max(1));
        }

        None
    }

    #[cfg(not(target_os = "linux"))]
    fn detect_cgroup_v2_cpu_limit() -> Option<usize> {
        None // cgroups are Linux-specific
    }

    #[cfg(not(target_os = "linux"))]
    fn detect_cgroup_v1_cpu_limit() -> Option<usize> {
        None // cgroups are Linux-specific
    }
}

/// Disk information.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiskInfo {
    /// Total disk space.
    pub total: u64,
    /// Available disk space.
    pub available: u64,
    /// Mount point.
    pub mount_point: String,
}

// Helper functions

/// Find project root by looking for common project files.
fn find_project_root(start: &Path) -> Option<PathBuf> {
    let markers = [
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "go.mod",
        "pom.xml",
        "build.gradle",
        "Gemfile",
        "composer.json",
        ".git",
    ];

    let mut current = start.to_path_buf();

    loop {
        for marker in &markers {
            if current.join(marker).exists() {
                return Some(current);
            }
        }

        if !current.pop() {
            break;
        }
    }

    None
}

/// Detect project type from files.
fn detect_project_type(root: &Path) -> ProjectType {
    if root.join("Cargo.toml").exists() {
        ProjectType::Rust
    } else if root.join("package.json").exists() {
        ProjectType::Node
    } else if root.join("pyproject.toml").exists() || root.join("setup.py").exists() {
        ProjectType::Python
    } else if root.join("go.mod").exists() {
        ProjectType::Go
    } else if root.join("pom.xml").exists() || root.join("build.gradle").exists() {
        ProjectType::Java
    } else if root.join("Gemfile").exists() {
        ProjectType::Ruby
    } else if root.join("composer.json").exists() {
        ProjectType::Php
    } else if root.join("*.csproj").exists() || root.join("*.sln").exists() {
        ProjectType::Dotnet
    } else {
        ProjectType::Unknown
    }
}

/// Detect project details.
fn detect_project_details(
    root: &Path,
    project_type: &ProjectType,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<usize>,
) {
    match project_type {
        ProjectType::Rust => (
            Some("cargo".to_string()),
            Some("Rust".to_string()),
            None,
            None,
        ),
        ProjectType::Node => {
            let pm = if root.join("pnpm-lock.yaml").exists() {
                "pnpm"
            } else if root.join("yarn.lock").exists() {
                "yarn"
            } else {
                "npm"
            };
            (
                Some(pm.to_string()),
                Some("JavaScript/TypeScript".to_string()),
                None,
                None,
            )
        }
        ProjectType::Python => {
            let pm = if root.join("poetry.lock").exists() {
                "poetry"
            } else if root.join("Pipfile.lock").exists() {
                "pipenv"
            } else {
                "pip"
            };
            (Some(pm.to_string()), Some("Python".to_string()), None, None)
        }
        ProjectType::Go => (Some("go".to_string()), Some("Go".to_string()), None, None),
        _ => (None, None, None, None),
    }
}

/// Detect installed tools.
async fn detect_tools() -> Vec<InstalledTool> {
    let tools = [
        "git", "docker", "kubectl", "npm", "yarn", "pnpm", "cargo", "rustc", "python", "python3",
        "pip", "pip3", "node", "go", "java", "ruby", "php", "dotnet", "make", "cmake", "gcc",
        "clang", "vim", "nvim",
    ];

    let mut installed = Vec::new();

    for tool in tools {
        if let Ok(path) = which::which(tool) {
            let version = detect_version(tool, &["--version"]);
            installed.push(InstalledTool {
                name: tool.to_string(),
                path: Some(path),
                version,
            });
        }
    }

    installed
}

/// Detect version of a tool.
fn detect_version(tool: &str, args: &[&str]) -> Option<String> {
    Command::new(tool)
        .args(args)
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.lines().next().map(|l| l.trim().to_string()))
}

/// Gather safe environment variables (exclude secrets).
fn gather_safe_env_vars() -> HashMap<String, String> {
    let safe_vars = [
        "PATH",
        "HOME",
        "USER",
        "SHELL",
        "TERM",
        "LANG",
        "LC_ALL",
        "EDITOR",
        "VISUAL",
        "PAGER",
        "PWD",
        "OLDPWD",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
    ];

    std::env::vars()
        .filter(|(k, _)| {
            safe_vars.contains(&k.as_str())
                || k.starts_with("CARGO_")
                || k.starts_with("RUST")
                || k.starts_with("NODE_")
                || k.starts_with("NPM_")
        })
        .filter(|(k, _)| {
            !k.contains("KEY")
                && !k.contains("SECRET")
                && !k.contains("TOKEN")
                && !k.contains("PASSWORD")
                && !k.contains("CREDENTIAL")
        })
        .collect()
}

/// Environment context builder.
pub struct EnvironmentContextBuilder {
    include_git: bool,
    include_project: bool,
    include_tools: bool,
    include_resources: bool,
    cwd: Option<PathBuf>,
}

impl EnvironmentContextBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            include_git: true,
            include_project: true,
            include_tools: true,
            include_resources: true,
            cwd: None,
        }
    }

    /// Set working directory.
    pub fn cwd(mut self, path: impl Into<PathBuf>) -> Self {
        self.cwd = Some(path.into());
        self
    }

    /// Include git context.
    pub fn git(mut self, include: bool) -> Self {
        self.include_git = include;
        self
    }

    /// Include project context.
    pub fn project(mut self, include: bool) -> Self {
        self.include_project = include;
        self
    }

    /// Include tools detection.
    pub fn tools(mut self, include: bool) -> Self {
        self.include_tools = include;
        self
    }

    /// Include system resources.
    pub fn resources(mut self, include: bool) -> Self {
        self.include_resources = include;
        self
    }

    /// Build the context.
    pub async fn build(self) -> Result<EnvironmentContext> {
        let cwd = self
            .cwd
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

        Ok(EnvironmentContext {
            os: OsInfo::detect(),
            shell: ShellInfo::detect(),
            cwd: cwd.clone(),
            user: UserInfo::detect(),
            project: if self.include_project {
                ProjectContext::detect(&cwd).await
            } else {
                None
            },
            git: if self.include_git {
                GitContext::detect(&cwd).await
            } else {
                None
            },
            runtime: RuntimeInfo::detect(),
            tools: if self.include_tools {
                detect_tools().await
            } else {
                Vec::new()
            },
            env_vars: gather_safe_env_vars(),
            resources: if self.include_resources {
                SystemResources::detect()
            } else {
                SystemResources {
                    cpu_cores: 0,
                    total_memory: 0,
                    available_memory: 0,
                    disk: DiskInfo::default(),
                }
            },
        })
    }
}

impl Default for EnvironmentContextBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_os_info() {
        let os = OsInfo::detect();
        assert!(!os.name.is_empty());
        assert!(!os.arch.is_empty());
    }

    #[test]
    fn test_shell_info() {
        let shell = ShellInfo::detect();
        assert!(!shell.name.is_empty());
    }

    #[test]
    fn test_user_info() {
        let user = UserInfo::detect();
        assert!(!user.username.is_empty());
    }

    #[test]
    fn test_project_type_display() {
        assert_eq!(format!("{}", ProjectType::Rust), "Rust");
        assert_eq!(format!("{}", ProjectType::Node), "Node.js");
    }

    #[tokio::test]
    async fn test_environment_context() {
        let ctx = EnvironmentContextBuilder::new()
            .tools(false) // Skip slow tool detection
            .build()
            .await
            .unwrap();

        assert!(!ctx.os.name.is_empty());
        assert!(!ctx.user.username.is_empty());
    }
}
