//! Terminal manager - manages multiple background terminals.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use uuid::Uuid;

use super::process::{BackgroundTerminal, LogLine, LogStream};

/// Terminal status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalStatus {
    Running,
    Stopped,
    Error,
}

/// Terminal information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalInfo {
    pub id: String,
    pub name: String,
    pub cwd: String,
    pub status: TerminalStatus,
    pub created_at: u64,
    pub exit_code: Option<i32>,
}

/// Terminal output event for streaming.
#[derive(Debug, Clone, Serialize)]
pub struct TerminalOutput {
    pub terminal_id: String,
    pub timestamp: u64,
    pub content: String,
    pub stream: String, // "stdout", "stderr", "system"
}

impl From<(String, LogLine)> for TerminalOutput {
    fn from((terminal_id, line): (String, LogLine)) -> Self {
        Self {
            terminal_id,
            timestamp: line.timestamp,
            content: line.content,
            stream: match line.stream {
                LogStream::Stdout => "stdout".to_string(),
                LogStream::Stderr => "stderr".to_string(),
                LogStream::System => "system".to_string(),
            },
        }
    }
}

/// Manages multiple background terminals.
pub struct TerminalManager {
    terminals: Arc<RwLock<HashMap<String, BackgroundTerminal>>>,
    /// Channel for terminal output events.
    output_tx: mpsc::UnboundedSender<TerminalOutput>,
    output_rx: Arc<RwLock<Option<mpsc::UnboundedReceiver<TerminalOutput>>>>,
}

impl TerminalManager {
    /// Create a new terminal manager.
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            terminals: Arc::new(RwLock::new(HashMap::new())),
            output_tx: tx,
            output_rx: Arc::new(RwLock::new(Some(rx))),
        }
    }

    /// Take the output receiver (can only be called once).
    pub async fn take_output_receiver(&self) -> Option<mpsc::UnboundedReceiver<TerminalOutput>> {
        self.output_rx.write().await.take()
    }

    /// Create a new background terminal.
    pub async fn create_terminal(&self, name: String, cwd: String) -> Result<TerminalInfo, String> {
        let id = Uuid::new_v4().to_string();

        let mut terminal = BackgroundTerminal::new(id.clone(), name.clone(), cwd.clone());

        // Set up output forwarding
        let (tx, mut rx) = mpsc::unbounded_channel::<LogLine>();
        terminal.set_output_channel(tx);

        // Start the terminal
        terminal.start()?;

        let info = TerminalInfo {
            id: id.clone(),
            name: terminal.name.clone(),
            cwd: terminal.cwd.clone(),
            status: TerminalStatus::Running,
            created_at: terminal.created_at,
            exit_code: None,
        };

        // Forward output to manager's channel
        let terminal_id = id.clone();
        let output_tx = self.output_tx.clone();
        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                let output = TerminalOutput::from((terminal_id.clone(), line));
                if output_tx.send(output).is_err() {
                    break;
                }
            }
        });

        self.terminals.write().await.insert(id, terminal);

        Ok(info)
    }

    /// Run a command in a terminal.
    pub async fn run_command(&self, terminal_id: &str, command: &str) -> Result<(), String> {
        let mut terminals = self.terminals.write().await;
        let terminal = terminals
            .get_mut(terminal_id)
            .ok_or_else(|| format!("Terminal not found: {}", terminal_id))?;

        terminal.run_command(command)
    }

    /// Get logs from a terminal.
    pub async fn get_logs(
        &self,
        terminal_id: &str,
        tail: Option<usize>,
    ) -> Result<Vec<TerminalOutput>, String> {
        let terminals = self.terminals.read().await;
        let terminal = terminals
            .get(terminal_id)
            .ok_or_else(|| format!("Terminal not found: {}", terminal_id))?;

        let tail = tail.unwrap_or(100);
        let logs = terminal.get_logs(tail);

        Ok(logs
            .into_iter()
            .map(|line| TerminalOutput::from((terminal_id.to_string(), line)))
            .collect())
    }

    /// List all terminals.
    pub async fn list_terminals(&self) -> Vec<TerminalInfo> {
        let mut terminals = self.terminals.write().await;

        terminals
            .values_mut()
            .map(|t| {
                t.check_status();
                TerminalInfo {
                    id: t.id.clone(),
                    name: t.name.clone(),
                    cwd: t.cwd.clone(),
                    status: if t.running {
                        TerminalStatus::Running
                    } else {
                        TerminalStatus::Stopped
                    },
                    created_at: t.created_at,
                    exit_code: t.exit_code,
                }
            })
            .collect()
    }

    /// Get terminal info.
    pub async fn get_terminal(&self, terminal_id: &str) -> Option<TerminalInfo> {
        let mut terminals = self.terminals.write().await;

        terminals.get_mut(terminal_id).map(|t| {
            t.check_status();
            TerminalInfo {
                id: t.id.clone(),
                name: t.name.clone(),
                cwd: t.cwd.clone(),
                status: if t.running {
                    TerminalStatus::Running
                } else {
                    TerminalStatus::Stopped
                },
                created_at: t.created_at,
                exit_code: t.exit_code,
            }
        })
    }

    /// Kill a terminal.
    pub async fn kill_terminal(&self, terminal_id: &str) -> Result<(), String> {
        let mut terminals = self.terminals.write().await;
        let terminal = terminals
            .get_mut(terminal_id)
            .ok_or_else(|| format!("Terminal not found: {}", terminal_id))?;

        terminal.kill()
    }

    /// Send SIGINT (Ctrl+C) to a terminal to interrupt the running process.
    pub async fn interrupt_terminal(&self, terminal_id: &str) -> Result<(), String> {
        let mut terminals = self.terminals.write().await;
        let terminal = terminals
            .get_mut(terminal_id)
            .ok_or_else(|| format!("Terminal not found: {}", terminal_id))?;

        terminal.interrupt()
    }

    /// Remove a terminal (must be stopped first).
    pub async fn remove_terminal(&self, terminal_id: &str) -> Result<(), String> {
        let mut terminals = self.terminals.write().await;

        // Check if stopped
        if let Some(terminal) = terminals.get_mut(terminal_id) {
            if terminal.running {
                return Err("Cannot remove running terminal. Kill it first.".to_string());
            }
        }

        terminals
            .remove(terminal_id)
            .map(|_| ())
            .ok_or_else(|| format!("Terminal not found: {}", terminal_id))
    }

    /// Kill all terminals. Used for graceful shutdown.
    pub async fn kill_all_terminals(&self) {
        let mut terminals = self.terminals.write().await;
        for (_, terminal) in terminals.iter_mut() {
            if terminal.running {
                let _ = terminal.kill();
            }
        }
        terminals.clear();
    }
}

impl Default for TerminalManager {
    fn default() -> Self {
        Self::new()
    }
}
