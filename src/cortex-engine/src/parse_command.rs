//! Command parsing for shell commands.
//!
//! Provides utilities for parsing, validating, and transforming shell commands
//! with support for various shell syntaxes and safety analysis.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A parsed shell command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedCommand {
    /// The executable/command name.
    pub executable: String,
    /// Command arguments.
    pub arguments: Vec<String>,
    /// Environment variables to set.
    pub env: HashMap<String, String>,
    /// Working directory.
    pub working_dir: Option<PathBuf>,
    /// Whether command uses pipes.
    pub has_pipes: bool,
    /// Whether command uses redirects.
    pub has_redirects: bool,
    /// Whether command runs in background.
    pub is_background: bool,
    /// Raw command string.
    pub raw: String,
    /// Command type.
    pub command_type: CommandType,
    /// Subcommands for pipelines.
    pub pipeline: Vec<PipelineSegment>,
}

impl ParsedCommand {
    /// Parse a command string.
    pub fn parse(command: &str) -> Result<Self, ParseError> {
        let parser = CommandParser::new();
        parser.parse(command)
    }

    /// Get the full command as a string.
    pub fn to_command_string(&self) -> String {
        if self.pipeline.is_empty() {
            let mut parts = vec![shell_escape(&self.executable)];
            parts.extend(self.arguments.iter().map(|a| shell_escape(a)));
            parts.join(" ")
        } else {
            self.raw.clone()
        }
    }

    /// Check if command modifies files.
    pub fn modifies_files(&self) -> bool {
        let write_commands = [
            "rm", "mv", "cp", "touch", "mkdir", "rmdir", "chmod", "chown", "dd", "mkfs",
            "truncate", "shred", "install",
        ];

        write_commands.contains(&self.executable.as_str())
            || self.has_redirects
            || self.arguments.iter().any(|a| a.starts_with('>'))
    }

    /// Check if command accesses network.
    pub fn accesses_network(&self) -> bool {
        let network_commands = [
            "curl",
            "wget",
            "ssh",
            "scp",
            "rsync",
            "nc",
            "netcat",
            "telnet",
            "ftp",
            "sftp",
            "ping",
            "traceroute",
            "nmap",
        ];

        network_commands.contains(&self.executable.as_str())
    }

    /// Check if command is potentially destructive.
    pub fn is_destructive(&self) -> bool {
        // Check for dangerous patterns
        if self.executable == "rm" {
            if self
                .arguments
                .iter()
                .any(|a| a.contains("-rf") || a.contains("-fr"))
            {
                return true;
            }
            if self
                .arguments
                .iter()
                .any(|a| a == "/" || a == "/*" || a.starts_with("/"))
            {
                return true;
            }
        }

        let destructive = ["mkfs", "dd", "shred", ":(){ :|:& };:"];
        destructive.iter().any(|d| self.raw.contains(d))
    }

    /// Get estimated execution time category.
    pub fn time_estimate(&self) -> TimeEstimate {
        match self.executable.as_str() {
            "ls" | "pwd" | "echo" | "cat" | "head" | "tail" => TimeEstimate::Instant,
            "grep" | "find" | "awk" | "sed" => TimeEstimate::Fast,
            "git" | "npm" | "cargo" | "pip" => TimeEstimate::Medium,
            "docker" | "make" | "gcc" | "rustc" => TimeEstimate::Slow,
            _ => TimeEstimate::Unknown,
        }
    }
}

/// Pipeline segment in a piped command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineSegment {
    /// Command in this segment.
    pub command: String,
    /// Arguments.
    pub arguments: Vec<String>,
    /// Redirect type if any.
    pub redirect: Option<Redirect>,
}

/// Redirect types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Redirect {
    /// Stdout to file (>).
    StdoutToFile(PathBuf),
    /// Stdout append to file (>>).
    StdoutAppend(PathBuf),
    /// Stderr to file (2>).
    StderrToFile(PathBuf),
    /// Both stdout and stderr (&>).
    AllToFile(PathBuf),
    /// Stdin from file (<).
    StdinFromFile(PathBuf),
    /// Here document (<<).
    HereDoc(String),
}

/// Command type classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandType {
    /// Simple command.
    Simple,
    /// Pipeline (cmd1 | cmd2).
    Pipeline,
    /// Sequential (cmd1; cmd2).
    Sequential,
    /// Conditional AND (cmd1 && cmd2).
    ConditionalAnd,
    /// Conditional OR (cmd1 || cmd2).
    ConditionalOr,
    /// Background (&).
    Background,
    /// Subshell ((...)).
    Subshell,
    /// Command substitution ($(...)).
    Substitution,
}

/// Time estimate for command execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimeEstimate {
    /// Less than 100ms.
    Instant,
    /// Less than 1 second.
    Fast,
    /// 1-10 seconds.
    Medium,
    /// More than 10 seconds.
    Slow,
    /// Cannot estimate.
    Unknown,
}

/// Parse error.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ParseError {
    #[error("Empty command")]
    EmptyCommand,

    #[error("Unclosed quote: {0}")]
    UnclosedQuote(char),

    #[error("Invalid escape sequence")]
    InvalidEscape,

    #[error("Unexpected token: {0}")]
    UnexpectedToken(String),

    #[error("Missing argument for: {0}")]
    MissingArgument(String),

    #[error("Syntax error: {0}")]
    SyntaxError(String),
}

/// Command parser.
#[allow(dead_code)]
pub struct CommandParser {
    /// Known shell builtins.
    builtins: Vec<&'static str>,
    /// Aliases.
    aliases: HashMap<String, String>,
}

impl CommandParser {
    /// Create a new parser.
    pub fn new() -> Self {
        Self {
            builtins: vec![
                "cd", "pwd", "echo", "export", "unset", "source", ".", "alias", "unalias", "set",
                "shopt", "history", "type", "which", "whereis", "read", "eval", "exec", "exit",
                "return", "break", "continue", "true", "false", "test", "[", "[[", "if", "then",
                "else", "elif", "fi", "case", "esac", "for", "while", "until", "do", "done",
                "function", "select", "time", "coproc", "{", "}", "!",
            ],
            aliases: HashMap::new(),
        }
    }

    /// Add an alias.
    pub fn add_alias(&mut self, name: &str, value: &str) {
        self.aliases.insert(name.to_string(), value.to_string());
    }

    /// Parse a command string.
    pub fn parse(&self, command: &str) -> Result<ParsedCommand, ParseError> {
        let command = command.trim();

        if command.is_empty() {
            return Err(ParseError::EmptyCommand);
        }

        // Tokenize
        let tokens = self.tokenize(command)?;

        if tokens.is_empty() {
            return Err(ParseError::EmptyCommand);
        }

        // Check for pipeline
        let has_pipes = tokens.iter().any(|t| t == "|");
        let has_redirects = tokens
            .iter()
            .any(|t| t == ">" || t == ">>" || t == "<" || t == "2>" || t == "&>");
        let is_background = tokens.last().map(|t| t == "&").unwrap_or(false);

        // Determine command type
        let command_type = self.determine_command_type(&tokens);

        // Parse pipeline if needed
        let pipeline = if has_pipes {
            self.parse_pipeline(&tokens)?
        } else {
            Vec::new()
        };

        // Extract executable and arguments
        let executable = self.resolve_alias(&tokens[0]);
        let arguments: Vec<String> = tokens[1..]
            .iter()
            .filter(|t| !is_operator(t))
            .cloned()
            .collect();

        Ok(ParsedCommand {
            executable,
            arguments,
            env: HashMap::new(),
            working_dir: None,
            has_pipes,
            has_redirects,
            is_background,
            raw: command.to_string(),
            command_type,
            pipeline,
        })
    }

    /// Tokenize a command string.
    fn tokenize(&self, command: &str) -> Result<Vec<String>, ParseError> {
        let mut tokens = Vec::new();
        let mut current = String::new();
        let mut chars = command.chars().peekable();
        let mut in_single_quote = false;
        let mut in_double_quote = false;
        let mut escape_next = false;

        while let Some(c) = chars.next() {
            if escape_next {
                current.push(c);
                escape_next = false;
                continue;
            }

            match c {
                '\\' if !in_single_quote => {
                    escape_next = true;
                }
                '\'' if !in_double_quote => {
                    in_single_quote = !in_single_quote;
                }
                '"' if !in_single_quote => {
                    in_double_quote = !in_double_quote;
                }
                ' ' | '\t' if !in_single_quote && !in_double_quote => {
                    if !current.is_empty() {
                        tokens.push(current.clone());
                        current.clear();
                    }
                }
                '|' | '&' | ';' | '>' | '<' if !in_single_quote && !in_double_quote => {
                    if !current.is_empty() {
                        tokens.push(current.clone());
                        current.clear();
                    }

                    // Handle multi-character operators
                    let mut op = c.to_string();
                    if let Some(&next) = chars.peek() {
                        match (c, next) {
                            ('|', '|') | ('&', '&') | ('>', '>') | ('2', '>') => {
                                chars.next();
                                op.push(next);
                            }
                            ('&', '>') => {
                                chars.next();
                                op.push(next);
                            }
                            _ => {}
                        }
                    }
                    tokens.push(op);
                }
                _ => {
                    current.push(c);
                }
            }
        }

        if in_single_quote {
            return Err(ParseError::UnclosedQuote('\''));
        }
        if in_double_quote {
            return Err(ParseError::UnclosedQuote('"'));
        }

        if !current.is_empty() {
            tokens.push(current);
        }

        Ok(tokens)
    }

    /// Determine command type from tokens.
    fn determine_command_type(&self, tokens: &[String]) -> CommandType {
        for token in tokens {
            match token.as_str() {
                "|" => return CommandType::Pipeline,
                "&&" => return CommandType::ConditionalAnd,
                "||" => return CommandType::ConditionalOr,
                ";" => return CommandType::Sequential,
                "&" => return CommandType::Background,
                _ => {}
            }
        }
        CommandType::Simple
    }

    /// Parse a pipeline into segments.
    fn parse_pipeline(&self, tokens: &[String]) -> Result<Vec<PipelineSegment>, ParseError> {
        let mut segments = Vec::new();
        let mut current_tokens = Vec::new();

        for token in tokens {
            if token == "|" {
                if current_tokens.is_empty() {
                    return Err(ParseError::SyntaxError("Empty pipeline segment".into()));
                }
                segments.push(self.tokens_to_segment(&current_tokens)?);
                current_tokens.clear();
            } else {
                current_tokens.push(token.clone());
            }
        }

        if !current_tokens.is_empty() {
            segments.push(self.tokens_to_segment(&current_tokens)?);
        }

        Ok(segments)
    }

    /// Convert tokens to a pipeline segment.
    fn tokens_to_segment(&self, tokens: &[String]) -> Result<PipelineSegment, ParseError> {
        if tokens.is_empty() {
            return Err(ParseError::EmptyCommand);
        }

        let mut redirect = None;
        let mut args = Vec::new();
        let mut i = 1;

        while i < tokens.len() {
            match tokens[i].as_str() {
                ">" => {
                    if i + 1 >= tokens.len() {
                        return Err(ParseError::MissingArgument(">".into()));
                    }
                    redirect = Some(Redirect::StdoutToFile(PathBuf::from(&tokens[i + 1])));
                    i += 2;
                }
                ">>" => {
                    if i + 1 >= tokens.len() {
                        return Err(ParseError::MissingArgument(">>".into()));
                    }
                    redirect = Some(Redirect::StdoutAppend(PathBuf::from(&tokens[i + 1])));
                    i += 2;
                }
                "<" => {
                    if i + 1 >= tokens.len() {
                        return Err(ParseError::MissingArgument("<".into()));
                    }
                    redirect = Some(Redirect::StdinFromFile(PathBuf::from(&tokens[i + 1])));
                    i += 2;
                }
                _ => {
                    args.push(tokens[i].clone());
                    i += 1;
                }
            }
        }

        Ok(PipelineSegment {
            command: tokens[0].clone(),
            arguments: args,
            redirect,
        })
    }

    /// Resolve an alias.
    fn resolve_alias(&self, name: &str) -> String {
        self.aliases
            .get(name)
            .cloned()
            .unwrap_or_else(|| name.to_string())
    }
}

impl Default for CommandParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Check if a token is an operator.
fn is_operator(token: &str) -> bool {
    matches!(
        token,
        "|" | "||" | "&&" | ";" | "&" | ">" | ">>" | "<" | "2>" | "&>"
    )
}

/// Escape a string for shell use.
pub fn shell_escape(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }

    // Check if escaping is needed
    let needs_escape = s.chars().any(|c| {
        matches!(
            c,
            ' ' | '\t'
                | '\n'
                | '"'
                | '\''
                | '\\'
                | '$'
                | '`'
                | '!'
                | '*'
                | '?'
                | '['
                | ']'
                | '{'
                | '}'
                | '|'
                | '&'
                | ';'
                | '<'
                | '>'
                | '('
                | ')'
                | '#'
                | '~'
        )
    });

    if !needs_escape {
        return s.to_string();
    }

    // Use single quotes for most cases
    if !s.contains('\'') {
        return format!("'{s}'");
    }

    // Use double quotes and escape
    let mut result = String::with_capacity(s.len() + 10);
    result.push('"');
    for c in s.chars() {
        match c {
            '"' | '\\' | '$' | '`' => {
                result.push('\\');
                result.push(c);
            }
            _ => result.push(c),
        }
    }
    result.push('"');
    result
}

/// Unescape a shell string.
pub fn shell_unescape(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    while let Some(c) = chars.next() {
        match c {
            '\\' if !in_single_quote => {
                if let Some(&next) = chars.peek() {
                    chars.next();
                    result.push(next);
                }
            }
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
            }
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
            }
            _ => result.push(c),
        }
    }

    result
}

/// Split a command string into words.
pub fn split_command(command: &str) -> Result<Vec<String>, ParseError> {
    let parser = CommandParser::new();
    parser.tokenize(command)
}

/// Join words into a command string.
pub fn join_command(words: &[String]) -> String {
    words
        .iter()
        .map(|w| shell_escape(w))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Command builder for constructing commands programmatically.
#[derive(Debug, Default)]
pub struct CommandBuilder {
    executable: String,
    arguments: Vec<String>,
    env: HashMap<String, String>,
    working_dir: Option<PathBuf>,
}

impl CommandBuilder {
    /// Create a new command builder.
    pub fn new(executable: impl Into<String>) -> Self {
        Self {
            executable: executable.into(),
            ..Default::default()
        }
    }

    /// Add an argument.
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.arguments.push(arg.into());
        self
    }

    /// Add multiple arguments.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.arguments
            .extend(args.into_iter().map(std::convert::Into::into));
        self
    }

    /// Add an environment variable.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Set working directory.
    pub fn working_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.working_dir = Some(path.into());
        self
    }

    /// Build the command string.
    pub fn build(&self) -> String {
        let mut parts = Vec::new();

        // Add environment variables
        for (k, v) in &self.env {
            parts.push(format!("{}={}", k, shell_escape(v)));
        }

        // Add cd if working directory is set
        if let Some(ref wd) = self.working_dir {
            parts.push(format!("cd {} &&", shell_escape(&wd.display().to_string())));
        }

        // Add command
        parts.push(shell_escape(&self.executable));

        // Add arguments
        for arg in &self.arguments {
            parts.push(shell_escape(arg));
        }

        parts.join(" ")
    }

    /// Build as a ParsedCommand.
    pub fn build_parsed(&self) -> ParsedCommand {
        ParsedCommand {
            executable: self.executable.clone(),
            arguments: self.arguments.clone(),
            env: self.env.clone(),
            working_dir: self.working_dir.clone(),
            has_pipes: false,
            has_redirects: false,
            is_background: false,
            raw: self.build(),
            command_type: CommandType::Simple,
            pipeline: Vec::new(),
        }
    }
}

/// Validate a command for safety.
pub fn validate_command(command: &str) -> CommandValidation {
    let parsed = match ParsedCommand::parse(command) {
        Ok(p) => p,
        Err(e) => return CommandValidation::invalid(e.to_string()),
    };

    let mut warnings = Vec::new();
    let mut errors = Vec::new();

    // Check for destructive patterns
    if parsed.is_destructive() {
        errors.push("Command contains potentially destructive operations".to_string());
    }

    // Check for network access
    if parsed.accesses_network() {
        warnings.push("Command accesses the network".to_string());
    }

    // Check for file modifications
    if parsed.modifies_files() {
        warnings.push("Command may modify files".to_string());
    }

    // Check for sudo
    if parsed.executable == "sudo" || parsed.arguments.contains(&"sudo".to_string()) {
        warnings.push("Command uses sudo/elevated privileges".to_string());
    }

    // Check for dangerous redirects
    if parsed.has_redirects {
        for token in &parsed.arguments {
            if token.contains(">/dev/") && !token.contains("/dev/null") {
                warnings.push("Command redirects to device file".to_string());
            }
        }
    }

    if !errors.is_empty() {
        return CommandValidation {
            valid: false,
            safe: false,
            errors,
            warnings,
            parsed: Some(parsed),
        };
    }

    CommandValidation {
        valid: true,
        safe: warnings.is_empty(),
        errors,
        warnings,
        parsed: Some(parsed),
    }
}

/// Command validation result.
#[derive(Debug, Clone)]
pub struct CommandValidation {
    /// Whether the command is syntactically valid.
    pub valid: bool,
    /// Whether the command is considered safe.
    pub safe: bool,
    /// Validation errors.
    pub errors: Vec<String>,
    /// Validation warnings.
    pub warnings: Vec<String>,
    /// Parsed command if valid.
    pub parsed: Option<ParsedCommand>,
}

impl CommandValidation {
    fn invalid(error: String) -> Self {
        Self {
            valid: false,
            safe: false,
            errors: vec![error],
            warnings: Vec::new(),
            parsed: None,
        }
    }
}

/// Extract file paths from a command.
pub fn extract_paths(command: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Ok(parsed) = ParsedCommand::parse(command) {
        for arg in &parsed.arguments {
            // Skip flags
            if arg.starts_with('-') {
                continue;
            }

            // Check if it looks like a path
            if arg.contains('/') || arg.contains('.') || Path::new(arg).exists() {
                paths.push(PathBuf::from(arg));
            }
        }
    }

    paths
}

/// Check if a command is a shell builtin.
pub fn is_builtin(command: &str) -> bool {
    let builtins = [
        "cd", "pwd", "echo", "export", "unset", "source", ".", "alias", "read", "eval", "exec",
        "exit", "return", "true", "false", "test",
    ];
    builtins.contains(&command)
}

/// Get the type of command (builtin, alias, function, file).
pub fn command_type(command: &str) -> &'static str {
    if is_builtin(command) {
        "builtin"
    } else if Path::new(command).is_absolute() {
        "file"
    } else {
        "external"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_command() {
        let cmd = ParsedCommand::parse("ls -la").unwrap();
        assert_eq!(cmd.executable, "ls");
        assert_eq!(cmd.arguments, vec!["-la"]);
        assert!(!cmd.has_pipes);
    }

    #[test]
    fn test_parse_pipeline() {
        let cmd = ParsedCommand::parse("cat file.txt | grep pattern | wc -l").unwrap();
        assert!(cmd.has_pipes);
        assert_eq!(cmd.command_type, CommandType::Pipeline);
        assert_eq!(cmd.pipeline.len(), 3);
    }

    #[test]
    fn test_parse_with_quotes() {
        let cmd = ParsedCommand::parse(r#"echo "hello world""#).unwrap();
        assert_eq!(cmd.executable, "echo");
        assert_eq!(cmd.arguments, vec!["hello world"]);
    }

    #[test]
    fn test_parse_redirect() {
        let cmd = ParsedCommand::parse("echo hello > file.txt").unwrap();
        assert!(cmd.has_redirects);
    }

    #[test]
    fn test_shell_escape() {
        assert_eq!(shell_escape("hello"), "hello");
        assert_eq!(shell_escape("hello world"), "'hello world'");
        assert_eq!(shell_escape(""), "''");
    }

    #[test]
    fn test_command_builder() {
        let cmd = CommandBuilder::new("git")
            .arg("commit")
            .arg("-m")
            .arg("my message")
            .build();
        assert_eq!(cmd, "git commit -m 'my message'");
    }

    #[test]
    fn test_validate_safe_command() {
        let result = validate_command("ls -la");
        assert!(result.valid);
        assert!(result.safe);
    }

    #[test]
    fn test_validate_dangerous_command() {
        let result = validate_command("rm -rf /");
        assert!(!result.safe);
        assert!(!result.errors.is_empty());
    }

    #[test]
    fn test_is_destructive() {
        let cmd = ParsedCommand::parse("rm -rf /tmp/test").unwrap();
        assert!(cmd.is_destructive());

        let cmd = ParsedCommand::parse("ls -la").unwrap();
        assert!(!cmd.is_destructive());
    }

    #[test]
    fn test_extract_paths() {
        let paths = extract_paths("cp file1.txt /tmp/file2.txt");
        assert_eq!(paths.len(), 2);
    }
}
