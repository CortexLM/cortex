//! Command patterns for safety analysis.

/// Commands that are always safe (read-only).
pub const SAFE_COMMANDS: &[&str] = &[
    // File viewing
    "cat",
    "head",
    "tail",
    "less",
    "more",
    "bat",
    "view",
    // Directory listing
    "ls",
    "ll",
    "la",
    "dir",
    "tree",
    "exa",
    "eza",
    // Search
    "grep",
    "rg",
    "ag",
    "ack",
    "find",
    "fd",
    "locate",
    "which",
    "whereis",
    "type",
    // Git read operations
    "git status",
    "git log",
    "git diff",
    "git show",
    "git branch",
    "git remote",
    "git tag",
    "git stash list",
    // System info
    "pwd",
    "date",
    "whoami",
    "hostname",
    "uname",
    "uptime",
    "df",
    "du",
    "free",
    "top",
    "htop",
    "ps",
    "pgrep",
    // Development tools (read-only)
    "cargo check",
    "cargo test --no-run",
    "npm ls",
    "npm outdated",
    "pip list",
    "pip show",
    "python --version",
    "node --version",
    "rustc --version",
    // Misc
    "echo",
    "printf",
    "true",
    "false",
    "test",
    "stat",
    "file",
    "wc",
    "sort",
    "uniq",
    "cut",
    "awk",
    "sed",
    "jq",
    "yq",
    "xargs",
];

/// Commands that are dangerous and should always require approval.
pub const DANGEROUS_COMMANDS: &[&str] = &[
    // Destructive
    "rm -rf /",
    "rm -rf ~",
    "rm -rf /*",
    "rm -rf .",
    "rm -rf ..",
    // System modification
    "mkfs",
    "fdisk",
    "dd if=",
    "format",
    // Privilege escalation
    "sudo su",
    "sudo -i",
    "sudo bash",
    // Code execution from internet
    "curl | bash",
    "curl | sh",
    "wget | bash",
    "wget | sh",
    // Dangerous git operations
    "git push --force",
    "git reset --hard",
    "git clean -fd",
];

/// Patterns that indicate write operations.
pub const WRITE_INDICATORS: &[&str] = &[
    ">",       // Redirect output
    ">>",      // Append output
    "tee",     // Write to file
    "mv",      // Move/rename
    "cp",      // Copy
    "mkdir",   // Create directory
    "touch",   // Create file
    "install", // Install
    "chmod",   // Change permissions
    "chown",   // Change ownership
    "ln",      // Create link
];

/// Commands that modify git repository.
pub const GIT_WRITE_COMMANDS: &[&str] = &[
    "git add",
    "git commit",
    "git push",
    "git pull",
    "git merge",
    "git rebase",
    "git checkout",
    "git reset",
    "git stash",
    "git cherry-pick",
    "git revert",
];
