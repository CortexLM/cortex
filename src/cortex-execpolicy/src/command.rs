//! Command parsing for the policy engine.

use crate::error::PolicyError;

/// A properly parsed command with separated program and arguments.
#[derive(Debug, Clone)]
pub struct ParsedCommand {
    /// The program/executable name.
    pub program: String,

    /// The base name of the program (without path).
    pub program_basename: String,

    /// All arguments (properly parsed from shell).
    pub args: Vec<String>,

    /// Raw command string (for pattern matching fallback).
    pub raw: String,

    /// Whether the command involves piping.
    pub has_pipe: bool,

    /// Whether the command has shell operators.
    pub has_shell_operators: bool,

    /// Detected subcommands (for multi-command strings).
    pub subcommands: Vec<ParsedCommand>,
}

impl ParsedCommand {
    /// Parse a command from string slice arguments.
    pub fn from_args(args: &[String]) -> Result<Self, PolicyError> {
        if args.is_empty() {
            return Err(PolicyError::InvalidCommand("empty command".to_string()));
        }

        let raw = args.join(" ");
        let program = args[0].clone();
        let program_basename = std::path::Path::new(&program)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&program)
            .to_string();

        let has_pipe = raw.contains('|');
        let has_shell_operators = raw.contains('|')
            || raw.contains(';')
            || raw.contains("&&")
            || raw.contains("||")
            || raw.contains('`')
            || raw.contains("$(");

        let mut subcommands = Vec::new();

        // Parse subcommands if shell operators detected
        if has_shell_operators {
            subcommands = Self::parse_subcommands(&raw)?;
        }

        Ok(Self {
            program,
            program_basename,
            args: args[1..].to_vec(),
            raw,
            has_pipe,
            has_shell_operators,
            subcommands,
        })
    }

    /// Parse a command from a raw shell string.
    pub fn from_shell_string(cmd: &str) -> Result<Self, PolicyError> {
        let trimmed = cmd.trim();
        if trimmed.is_empty() {
            return Err(PolicyError::InvalidCommand("empty command".to_string()));
        }

        // Use shlex for proper shell parsing
        let parts = match shlex::split(trimmed) {
            Some(parts) if !parts.is_empty() => parts,
            _ => {
                // Fallback to whitespace splitting if shlex fails
                trimmed.split_whitespace().map(String::from).collect()
            }
        };

        if parts.is_empty() {
            return Err(PolicyError::InvalidCommand(
                "no executable in command".to_string(),
            ));
        }

        Self::from_args(&parts)
    }

    /// Parse subcommands from a string with shell operators.
    fn parse_subcommands(raw: &str) -> Result<Vec<ParsedCommand>, PolicyError> {
        let mut commands = Vec::new();

        // Split by common shell operators
        let separators = [";", "&&", "||", "|"];

        for sep in separators {
            let parts: Vec<&str> = raw.split(sep).collect();
            if parts.len() > 1 {
                for part in parts {
                    let trimmed = part.trim();
                    if !trimmed.is_empty() {
                        // Try to parse each subcommand
                        if let Ok(parsed) = Self::from_shell_string(trimmed) {
                            commands.push(parsed);
                        }
                    }
                }
                break;
            }
        }

        Ok(commands)
    }

    /// Check if an argument is present (exact match).
    pub fn has_arg(&self, arg: &str) -> bool {
        self.args.iter().any(|a| a == arg)
    }

    /// Check if any argument starts with a prefix.
    pub fn has_arg_starting_with(&self, prefix: &str) -> bool {
        self.args.iter().any(|a| a.starts_with(prefix))
    }

    /// Check if any argument contains a substring.
    pub fn has_arg_containing(&self, substr: &str) -> bool {
        self.args.iter().any(|a| a.contains(substr))
    }

    /// Check if a flag is present (handles -x, --xxx, -xyz formats).
    pub fn has_flag(&self, short: Option<char>, long: Option<&str>) -> bool {
        for arg in &self.args {
            // Check long flag
            if let Some(l) = long
                && (arg == &format!("--{l}") || arg.starts_with(&format!("--{l}=")))
            {
                return true;
            }

            // Check short flag
            if let Some(s) = short {
                // Exact short flag: -x
                if arg == &format!("-{s}") {
                    return true;
                }
                // Combined short flags: -xyz (contains -x)
                if arg.starts_with('-')
                    && !arg.starts_with("--")
                    && arg.len() > 1
                    && arg[1..].contains(s)
                {
                    return true;
                }
            }
        }
        false
    }

    /// Get the value for a flag (handles --flag=value and --flag value).
    pub fn get_flag_value(&self, short: Option<char>, long: Option<&str>) -> Option<String> {
        let args = &self.args;

        for (i, arg) in args.iter().enumerate() {
            // Check long flag with =
            if let Some(l) = long {
                if let Some(value) = arg.strip_prefix(&format!("--{l}=")) {
                    return Some(value.to_string());
                }
                // Check long flag followed by value
                if arg == &format!("--{l}") && i + 1 < args.len() {
                    return Some(args[i + 1].clone());
                }
            }

            // Check short flag
            if let Some(s) = short
                && arg == &format!("-{s}")
                && i + 1 < args.len()
            {
                return Some(args[i + 1].clone());
            }
        }

        None
    }

    /// Get all positional arguments (non-flag arguments).
    /// Note: This is a heuristic and may not be 100% accurate for all commands.
    pub fn positional_args(&self) -> Vec<&str> {
        let mut positional = Vec::new();

        for arg in &self.args {
            // Skip any argument that looks like a flag
            if arg.starts_with('-') {
                continue;
            }

            positional.push(arg.as_str());
        }

        positional
    }
}
