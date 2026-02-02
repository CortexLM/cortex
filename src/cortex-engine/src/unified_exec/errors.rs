//! Execution errors.

use std::fmt;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Execution error.
#[derive(Debug)]
pub struct ExecError {
    /// Error kind.
    pub kind: ExecErrorKind,
    /// Error message.
    pub message: String,
    /// Command that failed.
    pub command: Option<String>,
    /// Exit code if available.
    pub exit_code: Option<i32>,
    /// Stderr output.
    pub stderr: Option<String>,
    /// Source error.
    pub source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl ExecError {
    /// Create a new execution error.
    pub fn new(kind: ExecErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            command: None,
            exit_code: None,
            stderr: None,
            source: None,
        }
    }

    /// Set command.
    pub fn with_command(mut self, command: impl Into<String>) -> Self {
        self.command = Some(command.into());
        self
    }

    /// Set exit code.
    pub fn with_exit_code(mut self, code: i32) -> Self {
        self.exit_code = Some(code);
        self
    }

    /// Set stderr.
    pub fn with_stderr(mut self, stderr: impl Into<String>) -> Self {
        self.stderr = Some(stderr.into());
        self
    }

    /// Set source error.
    pub fn with_source<E: std::error::Error + Send + Sync + 'static>(mut self, source: E) -> Self {
        self.source = Some(Box::new(source));
        self
    }

    /// Create a spawn error.
    pub fn spawn(command: &str, err: io::Error) -> Self {
        Self::new(ExecErrorKind::Spawn, err.to_string())
            .with_command(command)
            .with_source(err)
    }

    /// Create a timeout error.
    pub fn timeout(command: &str, duration_secs: u64) -> Self {
        Self::new(
            ExecErrorKind::Timeout,
            format!("Command timed out after {duration_secs} seconds"),
        )
        .with_command(command)
    }

    /// Create a signal error.
    pub fn signal(command: &str, signal: i32) -> Self {
        Self::new(
            ExecErrorKind::Signal,
            format!("Command killed by signal {signal}"),
        )
        .with_command(command)
    }

    /// Create an exit error.
    pub fn exit(command: &str, code: i32, stderr: Option<String>) -> Self {
        let mut err = Self::new(
            ExecErrorKind::NonZeroExit,
            format!("Command exited with code {code}"),
        )
        .with_command(command)
        .with_exit_code(code);

        if let Some(stderr) = stderr {
            err = err.with_stderr(stderr);
        }

        err
    }

    /// Create a working directory error.
    pub fn working_dir(path: &PathBuf, err: io::Error) -> Self {
        Self::new(
            ExecErrorKind::WorkingDirectory,
            format!("Invalid working directory: {}", path.display()),
        )
        .with_source(err)
    }

    /// Create a permission error.
    pub fn permission(command: &str) -> Self {
        Self::new(ExecErrorKind::Permission, "Permission denied").with_command(command)
    }

    /// Create a not found error.
    pub fn not_found(command: &str) -> Self {
        Self::new(
            ExecErrorKind::NotFound,
            format!("Command not found: {command}"),
        )
        .with_command(command)
    }

    /// Check if error is retriable.
    pub fn is_retriable(&self) -> bool {
        matches!(self.kind, ExecErrorKind::Timeout | ExecErrorKind::Signal)
    }
}

impl fmt::Display for ExecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)?;

        if let Some(ref cmd) = self.command {
            write!(f, " (command: {cmd})")?;
        }

        if let Some(code) = self.exit_code {
            write!(f, " (exit code: {code})")?;
        }

        Ok(())
    }
}

impl std::error::Error for ExecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|e| e.as_ref() as &(dyn std::error::Error + 'static))
    }
}

/// Execution error kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecErrorKind {
    /// Failed to spawn process.
    Spawn,
    /// Command timed out.
    Timeout,
    /// Command killed by signal.
    Signal,
    /// Command exited with non-zero code.
    NonZeroExit,
    /// Invalid working directory.
    WorkingDirectory,
    /// Permission denied.
    Permission,
    /// Command not found.
    NotFound,
    /// IO error.
    Io,
    /// Environment error.
    Environment,
    /// Shell error.
    Shell,
    /// Internal error.
    Internal,
}

impl fmt::Display for ExecErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn => write!(f, "Spawn error"),
            Self::Timeout => write!(f, "Timeout"),
            Self::Signal => write!(f, "Signal"),
            Self::NonZeroExit => write!(f, "Non-zero exit"),
            Self::WorkingDirectory => write!(f, "Working directory error"),
            Self::Permission => write!(f, "Permission denied"),
            Self::NotFound => write!(f, "Not found"),
            Self::Io => write!(f, "IO error"),
            Self::Environment => write!(f, "Environment error"),
            Self::Shell => write!(f, "Shell error"),
            Self::Internal => write!(f, "Internal error"),
        }
    }
}

impl From<io::Error> for ExecError {
    fn from(err: io::Error) -> Self {
        let kind = match err.kind() {
            io::ErrorKind::NotFound => ExecErrorKind::NotFound,
            io::ErrorKind::PermissionDenied => ExecErrorKind::Permission,
            io::ErrorKind::TimedOut => ExecErrorKind::Timeout,
            _ => ExecErrorKind::Io,
        };

        Self::new(kind, err.to_string()).with_source(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exec_error() {
        let err = ExecError::exit("ls", 1, Some("error".to_string()));
        assert_eq!(err.kind, ExecErrorKind::NonZeroExit);
        assert_eq!(err.exit_code, Some(1));
        assert_eq!(err.stderr, Some("error".to_string()));
    }

    #[test]
    fn test_error_display() {
        let err = ExecError::timeout("sleep 100", 10);
        let display = format!("{}", err);
        assert!(display.contains("timed out"));
    }

    #[test]
    fn test_retriable() {
        assert!(ExecError::timeout("cmd", 10).is_retriable());
        assert!(!ExecError::not_found("cmd").is_retriable());
    }
}
