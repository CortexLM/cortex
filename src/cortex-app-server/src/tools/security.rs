//! Security helper functions for command execution.

use std::path::{Path, PathBuf};

/// Parse a shell command string into program and arguments safely.
/// Does NOT use sh -c to prevent command injection via shell metacharacters.
pub fn parse_shell_command(command: &str) -> Result<(String, Vec<String>), String> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return Err("Empty command".to_string());
    }

    // Check for dangerous shell metacharacters that indicate injection attempts
    let dangerous_patterns = [
        "$(", "`", // Command substitution
        "&&", "||", // Command chaining
        ";",  // Command separator
        "|",  // Piping
        ">", ">>", "<", // Redirection
        "&", // Background execution
        "\n", "\r", // Newlines
        "\\", // Escape sequences
    ];

    for pattern in &dangerous_patterns {
        if trimmed.contains(pattern) {
            return Err(format!(
                "Command contains potentially dangerous character sequence: '{}'",
                pattern
            ));
        }
    }

    // Simple tokenization by whitespace (handles basic cases)
    // For quoted strings, we need more sophisticated parsing
    let tokens = tokenize_command(trimmed)?;

    if tokens.is_empty() {
        return Err("No command tokens found".to_string());
    }

    let program = tokens[0].clone();
    let args = tokens[1..].to_vec();

    Ok((program, args))
}

/// Simple command tokenizer that handles single and double quotes.
fn tokenize_command(input: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        match c {
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
            _ => {
                current.push(c);
            }
        }
        i += 1;
    }

    if in_single_quote || in_double_quote {
        return Err("Unclosed quote in command".to_string());
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    Ok(tokens)
}

/// Check if a command is potentially dangerous.
pub fn is_dangerous_command(program: &str) -> bool {
    // Extract just the program name (in case of full path)
    let program_name = std::path::Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(program);

    // List of dangerous commands that should be blocked
    let blocked_commands = [
        "rm",           // File deletion
        "rmdir",        // Directory deletion
        "mkfs",         // Filesystem formatting
        "dd",           // Direct disk access
        "format",       // Windows format
        "del",          // Windows delete
        "shutdown",     // System shutdown
        "reboot",       // System reboot
        "halt",         // System halt
        "poweroff",     // Power off
        "init",         // Init system
        "systemctl",    // System control
        "service",      // Service control
        "chmod",        // Permission changes (dangerous with recursive)
        "chown",        // Ownership changes
        "passwd",       // Password changes
        "useradd",      // User management
        "userdel",      // User deletion
        "groupadd",     // Group management
        "groupdel",     // Group deletion
        "sudo",         // Privilege escalation
        "su",           // Switch user
        "doas",         // OpenBSD privilege escalation
        "pkexec",       // PolicyKit execution
        "nc",           // Netcat (network tool)
        "ncat",         // Nmap netcat
        "netcat",       // Network tool
        "telnet",       // Telnet client
        "ssh",          // SSH client (could tunnel out)
        "scp",          // Secure copy
        "sftp",         // Secure FTP
        "ftp",          // FTP client
        "wget",         // Web download (could exfiltrate)
        "curl",         // HTTP client (allow in some contexts, block for security)
        "python",       // Can execute arbitrary code
        "python3",      // Can execute arbitrary code
        "perl",         // Can execute arbitrary code
        "ruby",         // Can execute arbitrary code
        "node",         // Can execute arbitrary code
        "php",          // Can execute arbitrary code
        "bash",         // Shell (injection vector)
        "sh",           // Shell (injection vector)
        "zsh",          // Shell (injection vector)
        "fish",         // Shell (injection vector)
        "csh",          // Shell (injection vector)
        "tcsh",         // Shell (injection vector)
        "ksh",          // Shell (injection vector)
        "dash",         // Shell (injection vector)
        "eval",         // Evaluate code
        "exec",         // Execute
        "source",       // Source file
        ".",            // Source file shorthand
        "crontab",      // Scheduled tasks
        "at",           // Scheduled tasks
        "nohup",        // Background process
        "screen",       // Terminal multiplexer
        "tmux",         // Terminal multiplexer
        "kill",         // Process termination
        "killall",      // Process termination
        "pkill",        // Process termination
        "mount",        // Mount filesystems
        "umount",       // Unmount filesystems
        "fdisk",        // Disk partitioning
        "parted",       // Disk partitioning
        "iptables",     // Firewall
        "nft",          // Nftables firewall
        "ufw",          // Ubuntu firewall
        "firewall-cmd", // Firewalld
    ];

    blocked_commands.contains(&program_name.to_lowercase().as_str())
}

/// Validate and resolve working directory.
pub fn validate_working_directory(requested: &Path, base: &Path) -> Result<PathBuf, String> {
    // Canonicalize paths to resolve symlinks and ..
    let base_canonical = base
        .canonicalize()
        .map_err(|e| format!("Cannot resolve base path: {}", e))?;

    // If the requested path is relative, join it with base
    let full_path = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        base.join(requested)
    };

    let requested_canonical = full_path
        .canonicalize()
        .map_err(|e| format!("Cannot resolve requested path: {}", e))?;

    // Check that the requested path is within or equal to the base path
    // This prevents directory traversal attacks
    if !requested_canonical.starts_with(&base_canonical) {
        return Err(format!(
            "Working directory '{}' is outside allowed base '{}'",
            requested_canonical.display(),
            base_canonical.display()
        ));
    }

    Ok(requested_canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_shell_command_simple() {
        let (prog, args) = parse_shell_command("echo hello").unwrap();
        assert_eq!(prog, "echo");
        assert_eq!(args, vec!["hello"]);
    }

    #[test]
    fn test_parse_shell_command_rejects_injection() {
        assert!(parse_shell_command("echo hello; rm -rf /").is_err());
        assert!(parse_shell_command("echo $(whoami)").is_err());
        assert!(parse_shell_command("echo `id`").is_err());
        assert!(parse_shell_command("cat /etc/passwd | grep root").is_err());
        assert!(parse_shell_command("echo hello && cat /etc/passwd").is_err());
    }

    #[test]
    fn test_is_dangerous_command() {
        assert!(is_dangerous_command("rm"));
        assert!(is_dangerous_command("/bin/rm"));
        assert!(is_dangerous_command("sudo"));
        assert!(is_dangerous_command("bash"));
        assert!(!is_dangerous_command("ls"));
        assert!(!is_dangerous_command("cat"));
        assert!(!is_dangerous_command("grep"));
    }

    #[test]
    fn test_tokenize_command_with_quotes() {
        let tokens = tokenize_command("echo 'hello world'").unwrap();
        assert_eq!(tokens, vec!["echo", "hello world"]);

        let tokens = tokenize_command("echo \"hello world\"").unwrap();
        assert_eq!(tokens, vec!["echo", "hello world"]);
    }
}
