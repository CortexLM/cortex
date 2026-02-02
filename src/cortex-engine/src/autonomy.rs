//! Autonomy system for Cortex CLI.
//!
//! Autonomy levels control how much the agent can do automatically without
//! user confirmation.
//!
//! Levels:
//! - Manual: All actions require user approval
//! - Low: File edits and read-only commands auto-approved
//! - Medium: + reversible commands (npm install, git commit, builds)
//! - High: + all commands except blocked dangerous patterns
//! - Unsafe: Skip all permission checks (dangerous!)

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// Risk level for commands (autonomy-specific).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum RiskLevel {
    /// Safe to execute without approval (read-only).
    Safe,
    /// Low risk - minor side effects.
    Low,
    /// Medium risk - modifies files/packages.
    #[default]
    Medium,
    /// High risk - potentially destructive.
    High,
    /// Critical - dangerous/irreversible.
    Critical,
}

/// Safety analysis result.
#[derive(Debug, Clone)]
pub struct SafetyAnalysis {
    /// Risk level.
    pub risk_level: RiskLevel,
    /// Reasons for the risk assessment.
    pub reasons: Vec<String>,
    /// The analyzed command.
    pub command: String,
}

/// Autonomy level for the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AutonomyLevel {
    /// All actions require user approval.
    #[default]
    Manual,
    /// File edits and read-only commands auto-approved.
    Low,
    /// + reversible commands (npm install, git commit, builds).
    Medium,
    /// + all commands except blocked dangerous patterns.
    High,
    /// Skip all permission checks (DANGEROUS!).
    #[serde(rename = "skip-permissions-unsafe")]
    SkipPermissionsUnsafe,
}

impl AutonomyLevel {
    /// Parse from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "manual" | "normal" | "off" => Some(Self::Manual),
            "low" => Some(Self::Low),
            "medium" | "med" => Some(Self::Medium),
            "high" => Some(Self::High),
            "skip-permissions-unsafe" | "unsafe" | "yolo" => Some(Self::SkipPermissionsUnsafe),
            _ => None,
        }
    }

    /// Get the next level (for cycling through modes).
    pub fn next(&self) -> Self {
        match self {
            Self::Manual => Self::Low,
            Self::Low => Self::Medium,
            Self::Medium => Self::High,
            Self::High => Self::Manual,
            Self::SkipPermissionsUnsafe => Self::Manual,
        }
    }

    /// Get display name.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Manual => "Manual",
            Self::Low => "Auto (Low)",
            Self::Medium => "Auto (Medium)",
            Self::High => "Auto (High)",
            Self::SkipPermissionsUnsafe => "UNSAFE (Skip Permissions)",
        }
    }

    /// Get short name for status bar.
    pub fn short_name(&self) -> &'static str {
        match self {
            Self::Manual => "Manual",
            Self::Low => "Low",
            Self::Medium => "Med",
            Self::High => "High",
            Self::SkipPermissionsUnsafe => "UNSAFE",
        }
    }

    /// Check if this level auto-approves a given risk level.
    pub fn auto_approves(&self, risk: RiskLevel) -> bool {
        match self {
            Self::Manual => false,
            Self::Low => matches!(risk, RiskLevel::Safe | RiskLevel::Low),
            Self::Medium => matches!(risk, RiskLevel::Safe | RiskLevel::Low | RiskLevel::Medium),
            Self::High => true, // Still subject to safety interlocks
            Self::SkipPermissionsUnsafe => true,
        }
    }

    /// Check if file edits are auto-approved at this level.
    pub fn auto_approves_file_edits(&self) -> bool {
        !matches!(self, Self::Manual)
    }

    /// Check if this is an auto-run mode.
    pub fn is_auto(&self) -> bool {
        !matches!(self, Self::Manual)
    }
}

impl std::fmt::Display for AutonomyLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

/// Autonomy decision for a specific action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutonomyDecision {
    /// Auto-approve the action.
    AutoApprove,
    /// Require user confirmation.
    RequireApproval { reason: String },
    /// Block the action entirely.
    Block { reason: String },
}

impl AutonomyDecision {
    /// Check if this decision allows the action without user input.
    pub fn is_auto_approved(&self) -> bool {
        matches!(self, Self::AutoApprove)
    }

    /// Check if this decision blocks the action.
    pub fn is_blocked(&self) -> bool {
        matches!(self, Self::Block { .. })
    }
}

/// Autonomy manager that makes decisions about what to auto-approve.
pub struct AutonomyManager {
    /// Current autonomy level.
    level: AutonomyLevel,
    /// Session allowlist (commands approved during this session).
    session_allowlist: HashSet<String>,
    /// Permanently blocked patterns (safety interlocks).
    blocked_patterns: Vec<BlockedPattern>,
}

/// A blocked command pattern.
#[derive(Debug, Clone)]
struct BlockedPattern {
    pattern: String,
    reason: String,
}

impl AutonomyManager {
    /// Create a new autonomy manager.
    pub fn new(level: AutonomyLevel) -> Self {
        Self {
            level,
            session_allowlist: HashSet::new(),
            blocked_patterns: Self::default_blocked_patterns(),
        }
    }

    /// Get default blocked patterns (safety interlocks).
    fn default_blocked_patterns() -> Vec<BlockedPattern> {
        vec![
            BlockedPattern {
                pattern: "rm -rf /".to_string(),
                reason: "Catastrophic system destruction".to_string(),
            },
            BlockedPattern {
                pattern: "rm -rf ~".to_string(),
                reason: "Home directory destruction".to_string(),
            },
            BlockedPattern {
                pattern: "dd of=/dev/".to_string(),
                reason: "Direct disk write".to_string(),
            },
            BlockedPattern {
                pattern: "mkfs".to_string(),
                reason: "Filesystem formatting".to_string(),
            },
            BlockedPattern {
                pattern: "> /dev/".to_string(),
                reason: "Direct device write".to_string(),
            },
            BlockedPattern {
                pattern: "chmod -R 777 /".to_string(),
                reason: "Catastrophic permission change".to_string(),
            },
            BlockedPattern {
                pattern: ":(){:|:&};:".to_string(),
                reason: "Fork bomb".to_string(),
            },
        ]
    }

    /// Get current autonomy level.
    pub fn level(&self) -> AutonomyLevel {
        self.level
    }

    /// Set autonomy level.
    pub fn set_level(&mut self, level: AutonomyLevel) {
        self.level = level;
    }

    /// Cycle to next autonomy level.
    pub fn cycle(&mut self) {
        self.level = self.level.next();
    }

    /// Decide whether to auto-approve a command execution.
    pub fn decide_command(&self, command: &str, analysis: &SafetyAnalysis) -> AutonomyDecision {
        // Check safety interlocks first (always block regardless of level)
        if !matches!(self.level, AutonomyLevel::SkipPermissionsUnsafe) {
            for pattern in &self.blocked_patterns {
                if command.contains(&pattern.pattern) {
                    return AutonomyDecision::Block {
                        reason: format!("Safety interlock: {}", pattern.reason),
                    };
                }
            }

            // Check for command substitution (always require approval)
            if self.has_command_substitution(command) {
                return AutonomyDecision::RequireApproval {
                    reason: "Command contains substitution ($(...) or backticks)".to_string(),
                };
            }
        }

        // Check session allowlist
        if self.session_allowlist.contains(command) {
            return AutonomyDecision::AutoApprove;
        }

        // Check autonomy level
        if self.level.auto_approves(analysis.risk_level) {
            AutonomyDecision::AutoApprove
        } else {
            AutonomyDecision::RequireApproval {
                reason: format!(
                    "Command risk ({:?}) exceeds autonomy level ({})",
                    analysis.risk_level,
                    self.level.display_name()
                ),
            }
        }
    }

    /// Decide whether to auto-approve a file edit.
    pub fn decide_file_edit(&self, _path: &str) -> AutonomyDecision {
        if self.level.auto_approves_file_edits() {
            AutonomyDecision::AutoApprove
        } else {
            AutonomyDecision::RequireApproval {
                reason: "File edits require approval in Manual mode".to_string(),
            }
        }
    }

    /// Decide whether to auto-approve a file creation.
    pub fn decide_file_create(&self, _path: &str) -> AutonomyDecision {
        if self.level.auto_approves_file_edits() {
            AutonomyDecision::AutoApprove
        } else {
            AutonomyDecision::RequireApproval {
                reason: "File creation requires approval in Manual mode".to_string(),
            }
        }
    }

    /// Add a command to the session allowlist.
    pub fn add_to_allowlist(&mut self, command: &str) {
        self.session_allowlist.insert(command.to_string());
    }

    /// Check if command contains substitution patterns.
    fn has_command_substitution(&self, command: &str) -> bool {
        command.contains("$(") || command.contains("`")
    }

    /// Clear the session allowlist.
    pub fn clear_allowlist(&mut self) {
        self.session_allowlist.clear();
    }
}

impl Default for AutonomyManager {
    fn default() -> Self {
        Self::new(AutonomyLevel::Manual)
    }
}

/// Risk classification for commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskClassification {
    /// Risk level.
    pub level: RiskLevel,
    /// Short justification.
    pub reason: String,
    /// Category of command.
    pub category: CommandCategory,
}

/// Command category for risk classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandCategory {
    /// Read-only operations (ls, cat, git status).
    ReadOnly,
    /// File system changes (edit, create, move).
    FileSystem,
    /// Package management (npm install, pip install).
    PackageManager,
    /// Git operations (commit, push).
    Git,
    /// Build tools (make, npm run build).
    Build,
    /// Network operations (curl, wget).
    Network,
    /// System operations (sudo, service).
    System,
    /// Dangerous operations (rm -rf, dd).
    Dangerous,
    /// Unknown category.
    Unknown,
}

impl CommandCategory {
    /// Get default risk level for this category.
    pub fn default_risk(&self) -> RiskLevel {
        match self {
            Self::ReadOnly => RiskLevel::Safe,
            Self::FileSystem => RiskLevel::Low,
            Self::PackageManager => RiskLevel::Medium,
            Self::Git => RiskLevel::Medium,
            Self::Build => RiskLevel::Medium,
            Self::Network => RiskLevel::Medium,
            Self::System => RiskLevel::High,
            Self::Dangerous => RiskLevel::Critical,
            Self::Unknown => RiskLevel::Medium,
        }
    }
}

/// Classify a command for risk assessment.
pub fn classify_command(command: &str) -> RiskClassification {
    let cmd_lower = command.to_lowercase();
    let first_word = command.split_whitespace().next().unwrap_or("");

    // Read-only commands
    let read_only = [
        "ls",
        "cat",
        "head",
        "tail",
        "less",
        "more",
        "grep",
        "find",
        "pwd",
        "whoami",
        "date",
        "uname",
        "ps",
        "top",
        "df",
        "du",
        "wc",
        "git status",
        "git log",
        "git diff",
        "git branch",
        "git show",
        "echo",
        "printf",
        "env",
        "printenv",
        "which",
        "type",
        "file",
    ];

    for ro in &read_only {
        if cmd_lower.starts_with(ro) {
            return RiskClassification {
                level: RiskLevel::Safe,
                reason: format!("Read-only command: {first_word}"),
                category: CommandCategory::ReadOnly,
            };
        }
    }

    // Dangerous commands
    let dangerous = [
        "rm -rf /",
        "rm -rf ~",
        "dd ",
        "mkfs",
        "fdisk",
        "parted",
        ":(){:|:&};:",
        "chmod -R 777 /",
        "> /dev/",
        "shutdown",
        "reboot",
        "init 0",
        "init 6",
        "halt",
        "poweroff",
    ];

    for d in &dangerous {
        if cmd_lower.contains(d) {
            return RiskClassification {
                level: RiskLevel::Critical,
                reason: format!("Dangerous command pattern: {d}"),
                category: CommandCategory::Dangerous,
            };
        }
    }

    // Package managers (medium risk)
    let pkg_managers = [
        "npm", "yarn", "pnpm", "pip", "pip3", "cargo", "go get", "gem", "bundle", "composer",
        "apt", "apt-get", "brew", "dnf", "yum",
    ];

    for pm in &pkg_managers {
        if first_word == *pm || cmd_lower.starts_with(pm) {
            return RiskClassification {
                level: RiskLevel::Medium,
                reason: format!("Package manager: {pm}"),
                category: CommandCategory::PackageManager,
            };
        }
    }

    // Git operations
    if first_word == "git" {
        if cmd_lower.contains("push") {
            return RiskClassification {
                level: RiskLevel::High,
                reason: "Git push modifies remote repository".to_string(),
                category: CommandCategory::Git,
            };
        }
        return RiskClassification {
            level: RiskLevel::Medium,
            reason: "Git operation".to_string(),
            category: CommandCategory::Git,
        };
    }

    // Build commands
    let build_cmds = [
        "make",
        "cmake",
        "ninja",
        "gradle",
        "mvn",
        "ant",
        "npm run",
        "yarn run",
        "pnpm run",
        "cargo build",
        "go build",
    ];

    for bc in &build_cmds {
        if cmd_lower.starts_with(bc) {
            return RiskClassification {
                level: RiskLevel::Medium,
                reason: format!("Build command: {first_word}"),
                category: CommandCategory::Build,
            };
        }
    }

    // System commands (high risk)
    let system_cmds = [
        "sudo",
        "su",
        "systemctl",
        "service",
        "chown",
        "chmod",
        "useradd",
        "userdel",
        "groupadd",
        "passwd",
        "visudo",
    ];

    for sc in &system_cmds {
        if first_word == *sc {
            return RiskClassification {
                level: RiskLevel::High,
                reason: format!("System command: {sc}"),
                category: CommandCategory::System,
            };
        }
    }

    // Network commands
    let network_cmds = ["curl", "wget", "ssh", "scp", "rsync", "nc", "netcat"];

    for nc in &network_cmds {
        if first_word == *nc {
            return RiskClassification {
                level: RiskLevel::Medium,
                reason: format!("Network command: {nc}"),
                category: CommandCategory::Network,
            };
        }
    }

    // File system operations
    let fs_cmds = ["rm", "mv", "cp", "mkdir", "rmdir", "touch", "ln"];

    for fc in &fs_cmds {
        if first_word == *fc {
            return RiskClassification {
                level: RiskLevel::Low,
                reason: format!("File system operation: {fc}"),
                category: CommandCategory::FileSystem,
            };
        }
    }

    // Default: medium risk unknown
    RiskClassification {
        level: RiskLevel::Medium,
        reason: "Unknown command category".to_string(),
        category: CommandCategory::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_autonomy_level_parsing() {
        assert_eq!(AutonomyLevel::from_str("low"), Some(AutonomyLevel::Low));
        assert_eq!(
            AutonomyLevel::from_str("medium"),
            Some(AutonomyLevel::Medium)
        );
        assert_eq!(AutonomyLevel::from_str("high"), Some(AutonomyLevel::High));
        assert_eq!(
            AutonomyLevel::from_str("manual"),
            Some(AutonomyLevel::Manual)
        );
        assert_eq!(
            AutonomyLevel::from_str("yolo"),
            Some(AutonomyLevel::SkipPermissionsUnsafe)
        );
    }

    #[test]
    fn test_autonomy_level_cycling() {
        let mut level = AutonomyLevel::Manual;
        level = level.next();
        assert_eq!(level, AutonomyLevel::Low);
        level = level.next();
        assert_eq!(level, AutonomyLevel::Medium);
        level = level.next();
        assert_eq!(level, AutonomyLevel::High);
        level = level.next();
        assert_eq!(level, AutonomyLevel::Manual);
    }

    #[test]
    fn test_risk_classification() {
        let class = classify_command("ls -la");
        assert_eq!(class.level, RiskLevel::Safe);
        assert_eq!(class.category, CommandCategory::ReadOnly);

        let class = classify_command("rm -rf /");
        assert_eq!(class.level, RiskLevel::Critical);
        assert_eq!(class.category, CommandCategory::Dangerous);

        let class = classify_command("npm install express");
        assert_eq!(class.level, RiskLevel::Medium);
        assert_eq!(class.category, CommandCategory::PackageManager);

        let class = classify_command("git push origin main");
        assert_eq!(class.level, RiskLevel::High);
        assert_eq!(class.category, CommandCategory::Git);
    }

    #[test]
    fn test_autonomy_manager() {
        let manager = AutonomyManager::new(AutonomyLevel::Low);

        // Low autonomy should auto-approve read-only
        let analysis = SafetyAnalysis {
            risk_level: RiskLevel::Safe,
            reasons: vec![],
            command: "ls".to_string(),
        };
        let decision = manager.decide_command("ls", &analysis);
        assert!(decision.is_auto_approved());

        // Low autonomy should require approval for medium risk
        let analysis = SafetyAnalysis {
            risk_level: RiskLevel::Medium,
            reasons: vec![],
            command: "npm install".to_string(),
        };
        let decision = manager.decide_command("npm install", &analysis);
        assert!(!decision.is_auto_approved());

        // Dangerous commands should be blocked
        let analysis = SafetyAnalysis {
            risk_level: RiskLevel::Critical,
            reasons: vec![],
            command: "rm -rf /".to_string(),
        };
        let decision = manager.decide_command("rm -rf /", &analysis);
        assert!(decision.is_blocked());
    }
}
