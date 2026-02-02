//! Shell handling and detection.
//!
//! Provides shell-specific functionality for bash, zsh, powershell,
//! and other shells.

use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::error::{CortexError, Result};

/// Shell type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShellType {
    /// Bourne Again Shell.
    Bash,
    /// Z Shell.
    Zsh,
    /// Fish shell.
    Fish,
    /// PowerShell.
    PowerShell,
    /// Windows Command Prompt.
    Cmd,
    /// Bourne Shell.
    Sh,
    /// Dash.
    Dash,
    /// Korn Shell.
    Ksh,
    /// C Shell.
    Csh,
    /// TC Shell.
    Tcsh,
    /// Nushell.
    Nu,
    /// Unknown shell.
    Unknown,
}

impl ShellType {
    /// Detect the current shell.
    pub fn detect() -> Self {
        // Try SHELL environment variable first
        if let Ok(shell) = env::var("SHELL") {
            return Self::from_path(&shell);
        }

        // On Windows, check for PowerShell
        #[cfg(windows)]
        {
            if env::var("PSModulePath").is_ok() {
                return Self::PowerShell;
            }
            if env::var("COMSPEC").is_ok() {
                return Self::Cmd;
            }
        }

        Self::Unknown
    }

    /// Create from shell path.
    pub fn from_path(path: &str) -> Self {
        let name = Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        Self::from_name(name)
    }

    /// Create from shell name.
    pub fn from_name(name: &str) -> Self {
        match name.to_lowercase().as_str() {
            "bash" | "bash.exe" => Self::Bash,
            "zsh" | "zsh.exe" => Self::Zsh,
            "fish" | "fish.exe" => Self::Fish,
            "powershell" | "pwsh" | "powershell.exe" | "pwsh.exe" => Self::PowerShell,
            "cmd" | "cmd.exe" => Self::Cmd,
            "sh" | "sh.exe" => Self::Sh,
            "dash" | "dash.exe" => Self::Dash,
            "ksh" | "ksh.exe" | "ksh93" => Self::Ksh,
            "csh" | "csh.exe" => Self::Csh,
            "tcsh" | "tcsh.exe" => Self::Tcsh,
            "nu" | "nu.exe" | "nushell" => Self::Nu,
            _ => Self::Unknown,
        }
    }

    /// Get the shell name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Zsh => "zsh",
            Self::Fish => "fish",
            Self::PowerShell => "powershell",
            Self::Cmd => "cmd",
            Self::Sh => "sh",
            Self::Dash => "dash",
            Self::Ksh => "ksh",
            Self::Csh => "csh",
            Self::Tcsh => "tcsh",
            Self::Nu => "nu",
            Self::Unknown => "unknown",
        }
    }

    /// Check if this is a POSIX-compatible shell.
    pub fn is_posix(&self) -> bool {
        matches!(
            self,
            Self::Bash | Self::Zsh | Self::Sh | Self::Dash | Self::Ksh
        )
    }

    /// Check if this is a Windows shell.
    pub fn is_windows(&self) -> bool {
        matches!(self, Self::PowerShell | Self::Cmd)
    }

    /// Get the command flag for running a command string.
    pub fn command_flag(&self) -> &'static str {
        match self {
            Self::PowerShell => "-Command",
            Self::Cmd => "/c",
            _ => "-c",
        }
    }

    /// Get the environment variable syntax.
    pub fn env_var_syntax(&self, name: &str) -> String {
        match self {
            Self::PowerShell => format!("$env:{name}"),
            Self::Cmd => format!("%{name}%"),
            _ => format!("${name}"),
        }
    }

    /// Get the path separator.
    pub fn path_separator(&self) -> char {
        match self {
            Self::Cmd | Self::PowerShell => ';',
            _ => ':',
        }
    }

    /// Escape a string for this shell.
    pub fn escape(&self, s: &str) -> String {
        match self {
            Self::PowerShell => escape_powershell(s),
            Self::Cmd => escape_cmd(s),
            Self::Fish => escape_fish(s),
            _ => escape_posix(s),
        }
    }

    /// Quote a string for this shell.
    pub fn quote(&self, s: &str) -> String {
        match self {
            Self::PowerShell => quote_powershell(s),
            Self::Cmd => quote_cmd(s),
            Self::Fish => quote_fish(s),
            _ => quote_posix(s),
        }
    }

    /// Get the comment prefix.
    pub fn comment_prefix(&self) -> &'static str {
        match self {
            Self::Cmd => "REM ",
            _ => "# ",
        }
    }

    /// Get the line continuation character.
    pub fn line_continuation(&self) -> &'static str {
        match self {
            Self::Cmd => "^",
            Self::PowerShell => "`",
            _ => "\\",
        }
    }
}

impl Default for ShellType {
    fn default() -> Self {
        Self::detect()
    }
}

impl std::fmt::Display for ShellType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Escape a string for POSIX shells.
fn escape_posix(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        match c {
            '\'' | '"' | '\\' | '$' | '`' | '!' | '*' | '?' | '[' | ']' | '{' | '}' | '(' | ')'
            | '<' | '>' | '|' | '&' | ';' | ' ' | '\t' | '\n' | '#' | '~' => {
                result.push('\\');
                result.push(c);
            }
            _ => result.push(c),
        }
    }
    result
}

/// Quote a string for POSIX shells.
fn quote_posix(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }

    // Check if quoting is needed
    if s.chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.' || c == '/')
    {
        return s.to_string();
    }

    // Use single quotes, escaping single quotes
    let escaped = s.replace('\'', "'\\''");
    format!("'{escaped}'")
}

/// Escape a string for PowerShell.
fn escape_powershell(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        match c {
            '`' | '$' | '"' | '\'' | '#' | '(' | ')' | '{' | '}' | '[' | ']' | '&' | '|' | ';'
            | '<' | '>' | ' ' | '\t' | '\n' => {
                result.push('`');
                result.push(c);
            }
            _ => result.push(c),
        }
    }
    result
}

/// Quote a string for PowerShell.
fn quote_powershell(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }

    // Use single quotes, doubling single quotes
    let escaped = s.replace('\'', "''");
    format!("'{escaped}'")
}

/// Escape a string for cmd.exe.
fn escape_cmd(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        match c {
            '^' | '&' | '|' | '<' | '>' | '(' | ')' | '%' | '"' | ' ' | '\t' => {
                result.push('^');
                result.push(c);
            }
            _ => result.push(c),
        }
    }
    result
}

/// Quote a string for cmd.exe.
fn quote_cmd(s: &str) -> String {
    if s.is_empty() {
        return "\"\"".to_string();
    }

    // Use double quotes, escaping special characters
    let escaped = s.replace('^', "^^").replace('"', "\"\"").replace('%', "%%");
    format!("\"{escaped}\"")
}

/// Escape a string for fish shell.
fn escape_fish(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        match c {
            '\\' | '\'' | '"' | '$' | '*' | '?' | '~' | '#' | '(' | ')' | '{' | '}' | '[' | ']'
            | '<' | '>' | '&' | '|' | ';' | ' ' | '\t' | '\n' => {
                result.push('\\');
                result.push(c);
            }
            _ => result.push(c),
        }
    }
    result
}

/// Quote a string for fish shell.
fn quote_fish(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }

    // Use single quotes, escaping single quotes and backslashes
    let escaped = s.replace('\\', "\\\\").replace('\'', "\\'");
    format!("'{escaped}'")
}

/// Shell information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellInfo {
    /// Shell type.
    pub shell_type: ShellType,
    /// Shell path.
    pub path: Option<PathBuf>,
    /// Shell version.
    pub version: Option<String>,
    /// Interactive mode.
    pub interactive: bool,
    /// Login shell.
    pub login: bool,
}

impl ShellInfo {
    /// Detect shell information.
    pub fn detect() -> Self {
        let shell_type = ShellType::detect();
        let path = env::var("SHELL").ok().map(PathBuf::from);
        let version = Self::get_version(&shell_type, path.as_deref());
        let interactive = env::var("PS1").is_ok() || env::var("PROMPT").is_ok();
        let login = env::var("LOGIN_SHELL").is_ok();

        Self {
            shell_type,
            path,
            version,
            interactive,
            login,
        }
    }

    /// Get shell version.
    fn get_version(shell: &ShellType, path: Option<&Path>) -> Option<String> {
        let program = path
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| shell.name().to_string());

        let output = match shell {
            ShellType::Bash => Command::new(&program).arg("--version").output().ok()?,
            ShellType::Zsh => Command::new(&program).arg("--version").output().ok()?,
            ShellType::Fish => Command::new(&program).arg("--version").output().ok()?,
            ShellType::PowerShell => Command::new(&program)
                .args(["-Command", "$PSVersionTable.PSVersion.ToString()"])
                .output()
                .ok()?,
            _ => return None,
        };

        String::from_utf8(output.stdout)
            .ok()
            .map(|s| s.lines().next().unwrap_or("").trim().to_string())
            .filter(|s| !s.is_empty())
    }
}

/// Shell command builder.
pub struct ShellCommand {
    /// Shell type.
    shell: ShellType,
    /// Command parts.
    parts: Vec<String>,
    /// Environment variables.
    env: HashMap<String, String>,
    /// Working directory.
    cwd: Option<PathBuf>,
}

impl ShellCommand {
    /// Create a new shell command.
    pub fn new(shell: ShellType) -> Self {
        Self {
            shell,
            parts: Vec::new(),
            env: HashMap::new(),
            cwd: None,
        }
    }

    /// Add a command part.
    pub fn arg(mut self, arg: &str) -> Self {
        self.parts.push(arg.to_string());
        self
    }

    /// Add multiple parts.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for arg in args {
            self.parts.push(arg.as_ref().to_string());
        }
        self
    }

    /// Set environment variable.
    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.env.insert(key.to_string(), value.to_string());
        self
    }

    /// Set working directory.
    pub fn cwd(mut self, dir: impl AsRef<Path>) -> Self {
        self.cwd = Some(dir.as_ref().to_path_buf());
        self
    }

    /// Build the command string.
    pub fn build(&self) -> String {
        let mut parts = Vec::new();

        // Add environment variables
        for (key, value) in &self.env {
            let env_set = match self.shell {
                ShellType::PowerShell => format!("$env:{}={};", key, self.shell.quote(value)),
                ShellType::Cmd => format!("set {key}={value} &&"),
                ShellType::Fish => format!("set -x {} {};", key, self.shell.quote(value)),
                _ => format!("{}={}", key, self.shell.quote(value)),
            };
            parts.push(env_set);
        }

        // Add cd if needed
        if let Some(cwd) = &self.cwd {
            let cd = match self.shell {
                ShellType::PowerShell => {
                    format!("Set-Location {};", self.shell.quote(&cwd.to_string_lossy()))
                }
                _ => format!("cd {} &&", self.shell.quote(&cwd.to_string_lossy())),
            };
            parts.push(cd);
        }

        // Add command parts
        let command_parts: Vec<_> = self
            .parts
            .iter()
            .enumerate()
            .map(|(i, p)| {
                if i == 0 {
                    p.clone()
                } else {
                    self.shell.quote(p)
                }
            })
            .collect();
        parts.push(command_parts.join(" "));

        parts.join(" ")
    }

    /// Execute the command.
    pub fn execute(&self) -> Result<std::process::Output> {
        let command_str = self.build();
        let shell_path = match self.shell {
            ShellType::Bash => "bash",
            ShellType::Zsh => "zsh",
            ShellType::Fish => "fish",
            ShellType::PowerShell => {
                #[cfg(windows)]
                {
                    "powershell.exe"
                }
                #[cfg(not(windows))]
                {
                    "pwsh"
                }
            }
            ShellType::Cmd => "cmd.exe",
            _ => "sh",
        };

        let mut cmd = Command::new(shell_path);
        cmd.arg(self.shell.command_flag()).arg(&command_str);

        if let Some(cwd) = &self.cwd {
            cmd.current_dir(cwd);
        }

        for (key, value) in &self.env {
            cmd.env(key, value);
        }

        cmd.output().map_err(CortexError::Io)
    }
}

/// Bash-specific utilities.
pub mod bash {

    /// Check if a command is a bash builtin.
    pub fn is_builtin(cmd: &str) -> bool {
        const BUILTINS: &[&str] = &[
            ".",
            ":",
            "[",
            "alias",
            "bg",
            "bind",
            "break",
            "builtin",
            "caller",
            "cd",
            "command",
            "compgen",
            "complete",
            "compopt",
            "continue",
            "declare",
            "dirs",
            "disown",
            "echo",
            "enable",
            "eval",
            "exec",
            "exit",
            "export",
            "false",
            "fc",
            "fg",
            "getopts",
            "hash",
            "help",
            "history",
            "jobs",
            "kill",
            "let",
            "local",
            "logout",
            "mapfile",
            "popd",
            "printf",
            "pushd",
            "pwd",
            "read",
            "readarray",
            "readonly",
            "return",
            "set",
            "shift",
            "shopt",
            "source",
            "suspend",
            "test",
            "times",
            "trap",
            "true",
            "type",
            "typeset",
            "ulimit",
            "umask",
            "unalias",
            "unset",
            "wait",
        ];
        BUILTINS.contains(&cmd)
    }

    /// Parse bash options.
    pub fn parse_options(script: &str) -> Vec<String> {
        let mut options = Vec::new();
        for line in script.lines() {
            let line = line.trim();
            if line.starts_with("set -") || line.starts_with("set +") {
                options.push(line.to_string());
            } else if line.starts_with("shopt ") {
                options.push(line.to_string());
            }
        }
        options
    }

    /// Generate bash completion script.
    pub fn completion_script(program: &str, commands: &[&str]) -> String {
        let script = format!(
            r#"_{program}_completions() {{
    local cur="${{COMP_WORDS[COMP_CWORD]}}"
    local commands="{commands}"
    COMPREPLY=( $(compgen -W "$commands" -- "$cur") )
}}
complete -F _{program}_completions {program}
"#,
            program = program,
            commands = commands.join(" ")
        );
        script
    }
}

/// PowerShell-specific utilities.
pub mod powershell {

    /// Check if a command is a PowerShell cmdlet.
    pub fn is_cmdlet(cmd: &str) -> bool {
        // Common cmdlet verbs
        const VERBS: &[&str] = &[
            "Add", "Clear", "Close", "Copy", "Enter", "Exit", "Find", "Format", "Get", "Hide",
            "Join", "Lock", "Move", "New", "Open", "Out", "Pop", "Push", "Read", "Remove",
            "Rename", "Reset", "Search", "Select", "Set", "Show", "Skip", "Split", "Start", "Stop",
            "Test", "Unlock", "Watch", "Write",
        ];

        for verb in VERBS {
            if cmd.starts_with(verb) && cmd.contains('-') {
                return true;
            }
        }
        false
    }

    /// Convert a bash command to PowerShell.
    pub fn from_bash(bash_cmd: &str) -> String {
        let replacements = [
            ("ls ", "Get-ChildItem "),
            ("cat ", "Get-Content "),
            ("rm ", "Remove-Item "),
            ("mv ", "Move-Item "),
            ("cp ", "Copy-Item "),
            ("mkdir ", "New-Item -ItemType Directory "),
            ("echo ", "Write-Output "),
            ("pwd", "Get-Location"),
            ("cd ", "Set-Location "),
            ("grep ", "Select-String "),
            ("find ", "Get-ChildItem -Recurse "),
            ("head ", "Get-Content -Head "),
            ("tail ", "Get-Content -Tail "),
            ("wc -l", "(Get-Content).Count"),
            ("curl ", "Invoke-WebRequest "),
            ("wget ", "Invoke-WebRequest -OutFile "),
        ];

        let mut result = bash_cmd.to_string();
        for (bash, ps) in replacements {
            result = result.replace(bash, ps);
        }

        // Convert environment variables - simple pattern matching
        let mut new_result = String::new();
        let chars = result.chars().peekable();
        for c in chars {
            if c == '$' {
                new_result.push_str("$env:");
            } else {
                new_result.push(c);
            }
        }
        result = new_result;

        result
    }

    /// Generate PowerShell profile entry.
    pub fn profile_entry(program: &str, alias: Option<&str>) -> String {
        let alias = alias.unwrap_or(program);
        format!("Set-Alias -Name {alias} -Value (Get-Command {program}).Source\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_type_from_name() {
        assert_eq!(ShellType::from_name("bash"), ShellType::Bash);
        assert_eq!(ShellType::from_name("zsh"), ShellType::Zsh);
        assert_eq!(
            ShellType::from_name("powershell.exe"),
            ShellType::PowerShell
        );
    }

    #[test]
    fn test_escape_posix() {
        assert_eq!(escape_posix("hello world"), "hello\\ world");
        assert_eq!(escape_posix("test$var"), "test\\$var");
    }

    #[test]
    fn test_quote_posix() {
        assert_eq!(quote_posix("hello"), "hello");
        assert_eq!(quote_posix("hello world"), "'hello world'");
        assert_eq!(quote_posix("it's"), "'it'\\''s'");
    }

    #[test]
    fn test_shell_command_build() {
        let cmd = ShellCommand::new(ShellType::Bash)
            .arg("echo")
            .arg("hello world");
        assert!(cmd.build().contains("echo"));
    }

    #[test]
    fn test_bash_builtins() {
        assert!(bash::is_builtin("cd"));
        assert!(bash::is_builtin("echo"));
        assert!(!bash::is_builtin("ls"));
    }

    #[test]
    fn test_powershell_cmdlet() {
        assert!(powershell::is_cmdlet("Get-ChildItem"));
        assert!(powershell::is_cmdlet("Set-Location"));
        assert!(!powershell::is_cmdlet("ls"));
    }
}
