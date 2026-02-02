//! Tests for shell module.

use crate::shell::*;

#[test]
fn test_shell_type_variants() {
    let bash = ShellType::Bash;
    let zsh = ShellType::Zsh;
    let fish = ShellType::Fish;
    let powershell = ShellType::PowerShell;
    let cmd = ShellType::Cmd;

    assert!(matches!(bash, ShellType::Bash));
    assert!(matches!(zsh, ShellType::Zsh));
    assert!(matches!(fish, ShellType::Fish));
    assert!(matches!(powershell, ShellType::PowerShell));
    assert!(matches!(cmd, ShellType::Cmd));
}

#[test]
fn test_shell_type_from_name() {
    assert_eq!(ShellType::from_name("bash"), ShellType::Bash);
    assert_eq!(ShellType::from_name("zsh"), ShellType::Zsh);
    assert_eq!(ShellType::from_name("fish"), ShellType::Fish);
    assert_eq!(ShellType::from_name("powershell"), ShellType::PowerShell);
    assert_eq!(ShellType::from_name("cmd"), ShellType::Cmd);
}

#[test]
#[cfg_attr(windows, ignore = "Unix shell paths not applicable on Windows")]
fn test_shell_type_from_path() {
    assert_eq!(ShellType::from_path("/bin/bash"), ShellType::Bash);
    assert_eq!(ShellType::from_path("/usr/bin/zsh"), ShellType::Zsh);
    assert_eq!(ShellType::from_path("/usr/local/bin/fish"), ShellType::Fish);
}

#[test]
fn test_shell_type_name() {
    assert_eq!(ShellType::Bash.name(), "bash");
    assert_eq!(ShellType::Zsh.name(), "zsh");
    assert_eq!(ShellType::Fish.name(), "fish");
    assert_eq!(ShellType::PowerShell.name(), "powershell");
    assert_eq!(ShellType::Cmd.name(), "cmd");
}

#[test]
fn test_shell_type_is_posix() {
    assert!(ShellType::Bash.is_posix());
    assert!(ShellType::Zsh.is_posix());
    assert!(ShellType::Sh.is_posix());
    assert!(!ShellType::PowerShell.is_posix());
    assert!(!ShellType::Cmd.is_posix());
}

#[test]
fn test_shell_type_is_windows() {
    assert!(!ShellType::Bash.is_windows());
    assert!(ShellType::PowerShell.is_windows());
    assert!(ShellType::Cmd.is_windows());
}

#[test]
fn test_shell_type_command_flag() {
    assert_eq!(ShellType::Bash.command_flag(), "-c");
    assert_eq!(ShellType::PowerShell.command_flag(), "-Command");
    assert_eq!(ShellType::Cmd.command_flag(), "/c");
}

#[test]
fn test_shell_type_env_var_syntax() {
    assert_eq!(ShellType::Bash.env_var_syntax("HOME"), "$HOME");
    assert_eq!(ShellType::PowerShell.env_var_syntax("HOME"), "$env:HOME");
    assert_eq!(ShellType::Cmd.env_var_syntax("HOME"), "%HOME%");
}

#[test]
fn test_shell_type_path_separator() {
    assert_eq!(ShellType::Bash.path_separator(), ':');
    assert_eq!(ShellType::PowerShell.path_separator(), ';');
    assert_eq!(ShellType::Cmd.path_separator(), ';');
}

#[test]
fn test_shell_type_quote() {
    let quoted = ShellType::Bash.quote("hello world");
    assert!(quoted.contains("hello world"));

    let quoted = ShellType::PowerShell.quote("hello world");
    assert!(quoted.contains("hello world"));
}

#[test]
fn test_shell_type_escape() {
    let escaped = ShellType::Bash.escape("hello world");
    assert!(escaped.contains("hello\\ world"));

    let escaped = ShellType::PowerShell.escape("hello world");
    assert!(escaped.contains('`'));
}

#[test]
fn test_shell_type_comment_prefix() {
    assert_eq!(ShellType::Bash.comment_prefix(), "# ");
    assert_eq!(ShellType::Cmd.comment_prefix(), "REM ");
}

#[test]
fn test_shell_type_line_continuation() {
    assert_eq!(ShellType::Bash.line_continuation(), "\\");
    assert_eq!(ShellType::PowerShell.line_continuation(), "`");
    assert_eq!(ShellType::Cmd.line_continuation(), "^");
}

#[test]
fn test_shell_info_detect() {
    let info = ShellInfo::detect();
    // Should have a shell_type
    let _ = info.shell_type.name();
}

#[test]
fn test_shell_command_new() {
    let cmd = ShellCommand::new(ShellType::Bash);
    let built = cmd.build();
    // Just verify it builds without panic
    let _ = built.len();
}

#[test]
fn test_shell_command_arg() {
    let cmd = ShellCommand::new(ShellType::Bash).arg("echo").arg("hello");

    let built = cmd.build();
    assert!(built.contains("echo"));
}

#[test]
fn test_shell_command_args() {
    let cmd = ShellCommand::new(ShellType::Bash)
        .arg("ls")
        .args(["-la", "-h"]);

    let built = cmd.build();
    assert!(built.contains("ls"));
}

#[test]
fn test_shell_command_env() {
    let cmd = ShellCommand::new(ShellType::Bash)
        .env("MY_VAR", "my_value")
        .arg("echo")
        .arg("test");

    let built = cmd.build();
    assert!(built.contains("MY_VAR"));
}

#[test]
#[cfg_attr(windows, ignore = "Unix path /tmp not available on Windows")]
fn test_shell_command_cwd() {
    let cmd = ShellCommand::new(ShellType::Bash).cwd("/tmp").arg("pwd");

    let built = cmd.build();
    assert!(built.contains("/tmp") || built.contains("cd"));
}

#[test]
fn test_bash_is_builtin() {
    assert!(bash::is_builtin("cd"));
    assert!(bash::is_builtin("echo"));
    assert!(bash::is_builtin("export"));
    assert!(!bash::is_builtin("ls"));
    assert!(!bash::is_builtin("grep"));
}

#[test]
fn test_powershell_is_cmdlet() {
    assert!(powershell::is_cmdlet("Get-ChildItem"));
    assert!(powershell::is_cmdlet("Set-Location"));
    assert!(!powershell::is_cmdlet("ls"));
}

#[test]
fn test_shell_type_display() {
    assert_eq!(format!("{}", ShellType::Bash), "bash");
    assert_eq!(format!("{}", ShellType::Zsh), "zsh");
}

#[test]
fn test_shell_type_default() {
    let shell = ShellType::default();
    // Should detect something (varies by environment)
    let _ = shell.name();
}

#[test]
fn test_shell_type_sh() {
    assert_eq!(ShellType::from_name("sh"), ShellType::Sh);
    assert_eq!(ShellType::Sh.name(), "sh");
    assert!(ShellType::Sh.is_posix());
}

#[test]
fn test_shell_type_dash() {
    assert_eq!(ShellType::from_name("dash"), ShellType::Dash);
    assert!(ShellType::Dash.is_posix());
}

#[test]
fn test_shell_type_ksh() {
    assert_eq!(ShellType::from_name("ksh"), ShellType::Ksh);
    assert!(ShellType::Ksh.is_posix());
}

#[test]
fn test_shell_type_unknown() {
    assert_eq!(
        ShellType::from_name("unknown_shell_xyz"),
        ShellType::Unknown
    );
    assert!(!ShellType::Unknown.is_posix());
    assert!(!ShellType::Unknown.is_windows());
}

#[test]
fn test_shell_type_quote_simple() {
    // Simple alphanumeric strings shouldn't need quoting in POSIX
    let quoted = ShellType::Bash.quote("hello");
    assert_eq!(quoted, "hello");
}

#[test]
fn test_shell_type_quote_empty() {
    let quoted = ShellType::Bash.quote("");
    assert_eq!(quoted, "''");
}

#[test]
fn test_shell_info_fields() {
    let info = ShellInfo::detect();
    // Check that fields are accessible
    let _ = info.shell_type;
    let _ = info.path;
    let _ = info.version;
    let _ = info.interactive;
    let _ = info.login;
}
