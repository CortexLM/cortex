//! Command safety analysis and approval flow.
//!
//! Analyzes shell commands to determine their risk level and whether
//! they require user approval before execution.

use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;

use cortex_protocol::AskForApproval;
use regex::Regex;

/// Risk level of a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskLevel {
    /// Safe to execute without approval.
    Safe,
    /// Medium risk - may modify files.
    Medium,
    /// High risk - may cause data loss or system changes.
    High,
    /// Critical - destructive or irreversible.
    Critical,
}

/// Result of command safety analysis.
#[derive(Debug, Clone)]
pub struct SafetyAnalysis {
    /// Risk level of the command.
    pub risk_level: RiskLevel,
    /// Reason for the risk level.
    pub reason: String,
    /// Whether approval is required.
    pub requires_approval: bool,
    /// Suggested safer alternative (if any).
    pub safer_alternative: Option<String>,
}

// Commands that are always safe (read-only)
static SAFE_COMMANDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let mut s = HashSet::new();
    s.insert("ls");
    s.insert("pwd");
    s.insert("echo");
    s.insert("cat");
    s.insert("head");
    s.insert("tail");
    s.insert("less");
    s.insert("more");
    s.insert("grep");
    s.insert("rg");
    s.insert("find");
    s.insert("fd");
    s.insert("which");
    s.insert("whereis");
    s.insert("type");
    s.insert("file");
    s.insert("stat");
    s.insert("wc");
    s.insert("diff");
    s.insert("date");
    s.insert("uptime");
    s.insert("whoami");
    s.insert("id");
    s.insert("env");
    s.insert("printenv");
    s.insert("uname");
    s.insert("hostname");
    s.insert("df");
    s.insert("du");
    s.insert("free");
    s.insert("top");
    s.insert("htop");
    s.insert("ps");
    s.insert("pgrep");
    s.insert("lsof");
    s.insert("man");
    s.insert("help");
    s.insert("tree");
    s.insert("realpath");
    s.insert("dirname");
    s.insert("basename");
    s.insert("jq");
    s.insert("yq");
    s.insert("xargs");
    s.insert("sort");
    s.insert("uniq");
    s.insert("cut");
    s.insert("awk");
    s.insert("sed"); // read mode only
    s.insert("tr");
    // Git read commands
    s.insert("git");
    s
});

// Commands that modify files (medium risk)
static MEDIUM_RISK_COMMANDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let mut s = HashSet::new();
    s.insert("touch");
    s.insert("mkdir");
    s.insert("cp");
    s.insert("mv");
    s.insert("ln");
    s.insert("chmod");
    s.insert("chown");
    s.insert("tar");
    s.insert("zip");
    s.insert("unzip");
    s.insert("gzip");
    s.insert("gunzip");
    // Build tools
    s.insert("npm");
    s.insert("yarn");
    s.insert("pnpm");
    s.insert("cargo");
    s.insert("go");
    s.insert("make");
    s.insert("cmake");
    s.insert("pip");
    s.insert("pip3");
    s.insert("poetry");
    s.insert("bundle");
    s.insert("gradle");
    s.insert("mvn");
    s.insert("rustc");
    s.insert("gcc");
    s.insert("g++");
    s.insert("clang");
    // Editors (non-interactive)
    s.insert("sed");
    s.insert("patch");
    s
});

// High risk commands
static HIGH_RISK_COMMANDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let mut s = HashSet::new();
    s.insert("rm");
    s.insert("rmdir");
    s.insert("shred");
    s.insert("dd");
    s.insert("mkfs");
    s.insert("fdisk");
    s.insert("parted");
    // System modification
    s.insert("sudo");
    s.insert("su");
    s.insert("doas");
    s.insert("systemctl");
    s.insert("service");
    s.insert("init");
    s.insert("shutdown");
    s.insert("reboot");
    s.insert("halt");
    s.insert("poweroff");
    // Network
    s.insert("iptables");
    s.insert("ufw");
    s.insert("firewall-cmd");
    // Package managers (system-wide)
    s.insert("apt");
    s.insert("apt-get");
    s.insert("yum");
    s.insert("dnf");
    s.insert("pacman");
    s.insert("brew");
    s.insert("snap");
    s.insert("flatpak");
    // Process control
    s.insert("kill");
    s.insert("killall");
    s.insert("pkill");
    // Git write operations
    s.insert("git-push");
    s
});

// Dangerous patterns
static DANGEROUS_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        // Only match rm -rf / or rm -rf ~ exactly, not any absolute path
        Regex::new(r"rm\s+(-[rRf]+\s+)*(/|~)\s*$").unwrap(),
        Regex::new(r"rm\\s+-rf\\s+/\\s*$").unwrap(),
        Regex::new(r">\s*/dev/sd[a-z]").unwrap(),
        Regex::new(r"dd\s+.*of=/dev/").unwrap(),
        Regex::new(r"mkfs").unwrap(),
        Regex::new(r":\(\)\{.*\}").unwrap(), // Fork bomb
        Regex::new(r"chmod\s+-R\s+777").unwrap(),
        Regex::new(r"curl.*\|\s*(ba)?sh").unwrap(),
        Regex::new(r"wget.*\|\s*(ba)?sh").unwrap(),
        Regex::new(r">\s*/etc/").unwrap(),
        Regex::new(r"eval\s+").unwrap(),
        Regex::new(r"\\$\(.*\)").unwrap(), // Command substitution (lower priority)
    ]
});

// Git commands that require approval
static GIT_WRITE_COMMANDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let mut s = HashSet::new();
    s.insert("push");
    s.insert("force-push");
    s.insert("reset");
    s.insert("rebase");
    s.insert("merge");
    s.insert("cherry-pick");
    s.insert("revert");
    s.insert("tag");
    s.insert("branch");
    s
});

/// Analyze a command for safety.
pub fn analyze_command(command: &[String], _cwd: &Path) -> SafetyAnalysis {
    if command.is_empty() {
        return SafetyAnalysis {
            risk_level: RiskLevel::Safe,
            reason: "Empty command".to_string(),
            requires_approval: false,
            safer_alternative: None,
        };
    }

    let cmd = &command[0];
    let cmd_name = Path::new(cmd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(cmd);

    let full_command = command.join(" ");

    // Check for dangerous patterns first
    for pattern in DANGEROUS_PATTERNS.iter() {
        if pattern.is_match(&full_command) {
            return SafetyAnalysis {
                risk_level: RiskLevel::Critical,
                reason: format!(
                    "Potentially dangerous pattern detected: {}",
                    pattern.as_str()
                ),
                requires_approval: true,
                safer_alternative: None,
            };
        }
    }

    // Check for high risk commands
    if HIGH_RISK_COMMANDS.contains(cmd_name) {
        let reason = match cmd_name {
            "rm" => {
                if command.iter().any(|a| a.contains("-r") || a.contains("-f")) {
                    "Recursive or forced file deletion".to_string()
                } else {
                    "File deletion".to_string()
                }
            }
            "sudo" | "su" | "doas" => "Elevated privileges required".to_string(),
            "kill" | "killall" | "pkill" => "Process termination".to_string(),
            _ => format!("High-risk command: {cmd_name}"),
        };

        return SafetyAnalysis {
            risk_level: RiskLevel::High,
            reason,
            requires_approval: true,
            safer_alternative: suggest_safer_alternative(cmd_name, command),
        };
    }

    // Check git commands
    if cmd_name == "git" && command.len() > 1 {
        let git_cmd = &command[1];
        if GIT_WRITE_COMMANDS.contains(git_cmd.as_str()) {
            return SafetyAnalysis {
                risk_level: RiskLevel::Medium,
                reason: format!("Git write operation: {git_cmd}"),
                requires_approval: true,
                safer_alternative: None,
            };
        }
        // Git read commands are safe
        return SafetyAnalysis {
            risk_level: RiskLevel::Safe,
            reason: "Git read operation".to_string(),
            requires_approval: false,
            safer_alternative: None,
        };
    }

    // Check for medium risk commands
    if MEDIUM_RISK_COMMANDS.contains(cmd_name) {
        let reason = match cmd_name {
            "mv" | "cp" => "File operation".to_string(),
            "chmod" | "chown" => "Permission change".to_string(),
            _ => format!("Build/modify command: {cmd_name}"),
        };

        return SafetyAnalysis {
            risk_level: RiskLevel::Medium,
            reason,
            requires_approval: false, // Medium risk doesn't require approval by default
            safer_alternative: None,
        };
    }

    // Safe commands
    if SAFE_COMMANDS.contains(cmd_name) {
        return SafetyAnalysis {
            risk_level: RiskLevel::Safe,
            reason: "Read-only operation".to_string(),
            requires_approval: false,
            safer_alternative: None,
        };
    }

    // Unknown command - treat as medium risk
    SafetyAnalysis {
        risk_level: RiskLevel::Medium,
        reason: format!("Unknown command: {cmd_name}"),
        requires_approval: false,
        safer_alternative: None,
    }
}

/// Suggest a safer alternative for a command.
fn suggest_safer_alternative(cmd: &str, _args: &[String]) -> Option<String> {
    match cmd {
        "rm" => {
            // Suggest using trash instead
            Some("Consider using 'trash' or 'mv' to a backup location instead".to_string())
        }
        _ => None,
    }
}

/// Check if a command requires approval based on policy.
pub fn requires_approval(analysis: &SafetyAnalysis, policy: &AskForApproval) -> bool {
    match policy {
        AskForApproval::Never => false,
        AskForApproval::OnFailure => false, // Only on failure
        AskForApproval::OnRequest => {
            // Model decides - approve high risk commands
            analysis.risk_level >= RiskLevel::High
        }
        AskForApproval::UnlessTrusted => {
            // Only auto-approve known safe commands
            analysis.risk_level > RiskLevel::Safe
        }
    }
}

/// Format a safety analysis for display.
pub fn format_analysis(analysis: &SafetyAnalysis) -> String {
    let risk_indicator = match analysis.risk_level {
        RiskLevel::Safe => "[OK] Safe",
        RiskLevel::Medium => "[WARN] Medium Risk",
        RiskLevel::High => "[WARN] High Risk",
        RiskLevel::Critical => "[CRITICAL] Critical Risk",
    };

    let mut output = format!("{}: {}", risk_indicator, analysis.reason);

    if let Some(ref alt) = analysis.safer_alternative {
        output.push_str(&format!("\n  Suggestion: {alt}"));
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_commands() {
        let analysis = analyze_command(&["ls".to_string(), "-la".to_string()], Path::new("/"));
        assert_eq!(analysis.risk_level, RiskLevel::Safe);
        assert!(!analysis.requires_approval);
    }

    #[test]
    fn test_rm_high_risk() {
        let analysis = analyze_command(
            &["rm".to_string(), "-rf".to_string(), "/tmp/test".to_string()],
            Path::new("/"),
        );
        assert_eq!(analysis.risk_level, RiskLevel::High);
        assert!(analysis.requires_approval);
    }

    #[test]
    fn test_git_push_requires_approval() {
        let analysis = analyze_command(&["git".to_string(), "push".to_string()], Path::new("/"));
        assert_eq!(analysis.risk_level, RiskLevel::Medium);
        assert!(analysis.requires_approval);
    }

    #[test]
    fn test_git_status_safe() {
        let analysis = analyze_command(&["git".to_string(), "status".to_string()], Path::new("/"));
        assert_eq!(analysis.risk_level, RiskLevel::Safe);
        assert!(!analysis.requires_approval);
    }

    #[test]
    fn test_dangerous_pattern() {
        let analysis = analyze_command(
            &["rm".to_string(), "-rf".to_string(), "/".to_string()],
            Path::new("/"),
        );
        assert_eq!(analysis.risk_level, RiskLevel::Critical);
    }
}
