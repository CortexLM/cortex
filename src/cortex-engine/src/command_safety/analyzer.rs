//! Command safety analyzer.

use super::patterns::*;

/// Risk level for a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    /// Safe to auto-execute (read-only).
    Safe,
    /// Low risk, minor side effects.
    Low,
    /// Medium risk, modifies files.
    Medium,
    /// High risk, potentially destructive.
    High,
    /// Critical risk, dangerous command.
    Critical,
}

/// Analysis result for a command.
#[derive(Debug, Clone)]
pub struct CommandAnalysis {
    /// Risk level.
    pub risk: RiskLevel,
    /// Whether the command is safe to auto-execute.
    pub is_safe: bool,
    /// Explanation of the analysis.
    pub explanation: String,
    /// Specific concerns identified.
    pub concerns: Vec<String>,
}

/// Check if a command is safe to auto-execute.
pub fn is_safe_command(command: &[String]) -> bool {
    if command.is_empty() {
        return false;
    }

    let analysis = analyze_command(command);
    analysis.is_safe
}

/// Analyze a command for safety.
pub fn analyze_command(command: &[String]) -> CommandAnalysis {
    if command.is_empty() {
        return CommandAnalysis {
            risk: RiskLevel::Safe,
            is_safe: false,
            explanation: "Empty command".to_string(),
            concerns: vec![],
        };
    }

    let cmd_str = command.join(" ");
    let program = &command[0];

    // Check for dangerous patterns first
    for dangerous in DANGEROUS_COMMANDS {
        if cmd_str.contains(dangerous) {
            return CommandAnalysis {
                risk: RiskLevel::Critical,
                is_safe: false,
                explanation: format!("Matches dangerous pattern: {dangerous}"),
                concerns: vec![format!("Contains dangerous pattern: {dangerous}")],
            };
        }
    }

    // Check for safe commands
    for safe in SAFE_COMMANDS {
        if cmd_str.starts_with(safe) || program == safe {
            // But check for write indicators even in "safe" commands
            if has_write_indicator(&cmd_str) {
                return CommandAnalysis {
                    risk: RiskLevel::Medium,
                    is_safe: false,
                    explanation: "Safe command with write operation".to_string(),
                    concerns: vec!["Contains output redirection or write operation".to_string()],
                };
            }

            return CommandAnalysis {
                risk: RiskLevel::Safe,
                is_safe: true,
                explanation: format!("{program} is a known safe command"),
                concerns: vec![],
            };
        }
    }

    // Check for git write commands
    for git_cmd in GIT_WRITE_COMMANDS {
        if cmd_str.starts_with(git_cmd) {
            let risk = if cmd_str.contains("--force") || cmd_str.contains("-f") {
                RiskLevel::High
            } else {
                RiskLevel::Medium
            };

            return CommandAnalysis {
                risk,
                is_safe: false,
                explanation: format!("{git_cmd} modifies repository"),
                concerns: vec!["Modifies git repository".to_string()],
            };
        }
    }

    // Check for write indicators
    if has_write_indicator(&cmd_str) {
        return CommandAnalysis {
            risk: RiskLevel::Medium,
            is_safe: false,
            explanation: "Command appears to write to files".to_string(),
            concerns: vec!["Contains write operation".to_string()],
        };
    }

    // Check for common build/install commands
    let build_commands = ["make", "cargo build", "cargo run", "npm run", "npm install", "pip install", "yarn"];
    for build_cmd in build_commands {
        if cmd_str.starts_with(build_cmd) || program == build_cmd {
            return CommandAnalysis {
                risk: RiskLevel::Low,
                is_safe: false,
                explanation: format!("{program} is a build/install command"),
                concerns: vec!["May modify project files".to_string()],
            };
        }
    }

    // Default: unknown command, medium risk
    CommandAnalysis {
        risk: RiskLevel::Medium,
        is_safe: false,
        explanation: format!("Unknown command: {program}"),
        concerns: vec!["Command not recognized as safe".to_string()],
    }
}

fn has_write_indicator(cmd: &str) -> bool {
    for indicator in WRITE_INDICATORS {
        if cmd.contains(indicator) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_commands() {
        assert!(is_safe_command(&["ls".to_string()]));
        assert!(is_safe_command(&["cat".to_string(), "file.txt".to_string()]));
        assert!(is_safe_command(&["git".to_string(), "status".to_string()]));
        assert!(is_safe_command(&["pwd".to_string()]));
    }

    #[test]
    fn test_unsafe_commands() {
        assert!(!is_safe_command(&["rm".to_string(), "-rf".to_string(), "/".to_string()]));
        assert!(!is_safe_command(&["git".to_string(), "push".to_string()]));
        assert!(!is_safe_command(&["npm".to_string(), "install".to_string()]));
    }

    #[test]
    fn test_write_indicators() {
        assert!(!is_safe_command(&["echo".to_string(), "test".to_string(), ">".to_string(), "file".to_string()]));
    }

    #[test]
    fn test_analyze_command() {
        let analysis = analyze_command(&["rm".to_string(), "-rf".to_string(), "/".to_string()]);
        assert_eq!(analysis.risk, RiskLevel::Critical);
        assert!(!analysis.is_safe);
    }
}
