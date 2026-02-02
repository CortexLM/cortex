//! Slash command handlers.

use crate::app::AppView;

use anyhow::Result;

use super::ActionHandler;

impl<'a> ActionHandler<'a> {
    /// Handle slash command - parse and execute a slash command.
    ///
    /// Supported commands:
    /// - `/help` - Show help
    /// - `/clear` - Clear messages
    /// - `/model <name>` - Switch model
    /// - `/quit` - Quit application
    pub(crate) async fn handle_slash_command(&mut self, cmd: &str) -> Result<bool> {
        tracing::debug!("Slash command: {}", cmd);

        let cmd = cmd.trim();
        let parts: Vec<&str> = cmd.splitn(2, ' ').collect();
        let command = parts
            .first()
            .map(|s| s.trim_start_matches('/'))
            .unwrap_or("");

        match command {
            "help" | "h" | "?" => {
                self.state.set_view(AppView::Help);
            }
            "clear" => {
                self.state.clear_messages();
            }
            "model" | "m" => {
                if let Some(model_name) = parts.get(1) {
                    if let Some(session) = self.session {
                        session.switch_model(model_name.to_string()).await?;
                        self.state.model = model_name.to_string();
                    }
                } else {
                    // Show model picker
                    self.handle_switch_model()?;
                }
            }
            "quit" | "q" | "exit" => {
                return self.handle_quit().await;
            }
            "new" | "n" => {
                return self.handle_new_session().await;
            }
            "compact" => {
                if let Some(session) = self.session {
                    session.compact().await?;
                }
            }
            "undo" | "u" => {
                if let Some(session) = self.session {
                    session.undo().await?;
                }
            }
            "redo" | "r" => {
                if let Some(session) = self.session {
                    session.redo().await?;
                }
            }
            _ => {
                tracing::warn!("Unknown command: {}", command);
                // Could show an error message to the user
            }
        }

        Ok(true)
    }
}
