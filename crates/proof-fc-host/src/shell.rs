//! Host commands behind a trait, so every jail / network / cleanup step is
//! rendered as an argv the tests can assert on without running anything.

use std::sync::Mutex;

use async_trait::async_trait;
use proof_vm_agent::HvError;

/// One finished command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdOutput {
    /// Exit status (`None` = killed by signal).
    pub code: Option<i32>,
    /// Stdout, lossy UTF-8.
    pub stdout: String,
    /// Stderr, lossy UTF-8.
    pub stderr: String,
}

impl CmdOutput {
    /// `Ok` iff the command exited 0.
    ///
    /// # Errors
    ///
    /// [`HvError::Backend`] naming the program and the stderr tail.
    pub fn ok(self, program: &str) -> Result<Self, HvError> {
        if self.code == Some(0) {
            Ok(self)
        } else {
            let tail: String = self
                .stderr
                .chars()
                .rev()
                .take(300)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            Err(HvError::Backend(format!(
                "{program} exited {:?}: {}",
                self.code,
                tail.trim()
            )))
        }
    }
}

/// Runs host commands. Production: [`SystemShell`]. Tests: [`RecordingShell`].
#[async_trait]
pub trait Shell: Send + Sync {
    /// Run `program` with `args` to completion.
    async fn run(&self, program: &str, args: &[String]) -> Result<CmdOutput, HvError>;
}

/// `tokio::process::Command`.
pub struct SystemShell;

#[async_trait]
impl Shell for SystemShell {
    async fn run(&self, program: &str, args: &[String]) -> Result<CmdOutput, HvError> {
        let out = tokio::process::Command::new(program)
            .args(args)
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|e| HvError::Backend(format!("spawn {program}: {e}")))?;
        Ok(CmdOutput {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// Records every argv and answers success. Never touches the host.
#[derive(Default)]
pub struct RecordingShell {
    calls: Mutex<Vec<Vec<String>>>,
}

impl RecordingShell {
    /// Every command run so far, program first.
    #[must_use]
    pub fn calls(&self) -> Vec<Vec<String>> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl Shell for RecordingShell {
    async fn run(&self, program: &str, args: &[String]) -> Result<CmdOutput, HvError> {
        let mut recorded = vec![program.to_owned()];
        recorded.extend(args.iter().cloned());
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(recorded);
        Ok(CmdOutput {
            code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

/// Convenience: run and require exit 0.
pub async fn sh(shell: &dyn Shell, program: &str, args: &[&str]) -> Result<CmdOutput, HvError> {
    let owned: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
    shell.run(program, &owned).await?.ok(program)
}
