//! Shell scripts for capturing and restoring state.

use super::{EXCLUDED_EXPORT_VARS, ShellType};

/// Get the capture script for a shell type.
pub fn capture_script(shell: ShellType) -> &'static str {
    match shell {
        ShellType::Zsh => ZSH_CAPTURE_SCRIPT,
        ShellType::Bash => BASH_CAPTURE_SCRIPT,
        ShellType::Sh => SH_CAPTURE_SCRIPT,
        _ => "",
    }
}

/// Zsh capture script.
pub const ZSH_CAPTURE_SCRIPT: &str = r##"
# Source user's zshrc first
if [[ -n "$ZDOTDIR" ]]; then
    rc="$ZDOTDIR/.zshrc"
else
    rc="$HOME/.zshrc"
fi
[[ -r "$rc" ]] && . "$rc"

# Begin snapshot
print '# Cortex Shell Snapshot (zsh)'
print "# Captured: $(date -Iseconds)"
print ''

# Unset all aliases first to avoid conflicts
print '# Clear existing aliases'
print 'unalias -a 2>/dev/null || true'
print ''

# Export functions
print '# Functions'
functions
print ''

# Shell options
print '# Shell options'
setopt | sed 's/^/setopt /'
print ''

# Aliases
print '# Aliases'
alias -L
print ''

# Environment variables (filtered, excludes PWD, OLDPWD, etc.)
print '# Environment variables'
export -p | grep -v -E '^export (PWD|OLDPWD|_|SHLVL|RANDOM|LINENO|SECONDS|HISTCMD|BASH_COMMAND|COLUMNS|LINES|SSH_AUTH_SOCK|SSH_AGENT_PID|GPG_AGENT_INFO)='
"##;

/// Bash capture script.
pub const BASH_CAPTURE_SCRIPT: &str = r##"
# Source user's bashrc first
if [ -z "$BASH_ENV" ] && [ -r "$HOME/.bashrc" ]; then
    . "$HOME/.bashrc"
fi

# Begin snapshot
echo '# Cortex Shell Snapshot (bash)'
echo "# Captured: $(date -Iseconds)"
echo ''

# Export functions
echo '# Functions'
declare -f
echo ''

# Shell options
echo '# Shell options'
set -o | awk '$2=="on"{print "set -o " $1}'
shopt | awk '$2=="on"{print "shopt -s " $1}'
echo ''

# Aliases
echo '# Aliases'
alias -p
echo ''

# Environment variables (filtered, excludes PWD, OLDPWD, etc.)
echo '# Environment variables'
export -p | grep -v -E '^(declare -x |export )?(PWD|OLDPWD|_|SHLVL|RANDOM|LINENO|SECONDS|HISTCMD|BASH_COMMAND|COLUMNS|LINES|SSH_AUTH_SOCK|SSH_AGENT_PID|GPG_AGENT_INFO)='
"##;

/// POSIX sh capture script (minimal, more compatible).
pub const SH_CAPTURE_SCRIPT: &str = r##"
# Begin snapshot
echo '# Cortex Shell Snapshot (sh)'
echo "# Captured: $(date)"
echo ''

# Aliases (if supported)
echo '# Aliases'
alias 2>/dev/null || true
echo ''

# Environment variables (filtered, excludes PWD, OLDPWD, etc.)
echo '# Environment variables'
export -p | grep -v -E '(PWD|OLDPWD|_|SHLVL|COLUMNS|LINES)='
"##;

/// Generate a restore script header.
pub fn restore_header(shell: ShellType) -> String {
    match shell {
        ShellType::Zsh => "#!/bin/zsh\n# Cortex Shell Snapshot Restore\n".to_string(),
        ShellType::Bash => "#!/bin/bash\n# Cortex Shell Snapshot Restore\n".to_string(),
        ShellType::Sh => "#!/bin/sh\n# Cortex Shell Snapshot Restore\n".to_string(),
        _ => "# Cortex Shell Snapshot Restore\n".to_string(),
    }
}

/// Get the filter for excluded variables.
pub fn excluded_vars_filter() -> String {
    EXCLUDED_EXPORT_VARS
        .iter()
        .map(|v| format!("exclude[\"{v}\"]=1"))
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capture_script_not_empty() {
        assert!(!capture_script(ShellType::Zsh).is_empty());
        assert!(!capture_script(ShellType::Bash).is_empty());
        assert!(!capture_script(ShellType::Sh).is_empty());
    }

    #[test]
    fn test_restore_header() {
        assert!(restore_header(ShellType::Zsh).contains("zsh"));
        assert!(restore_header(ShellType::Bash).contains("bash"));
    }
}
