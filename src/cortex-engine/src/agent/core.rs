//! Core agent implementation.

use std::sync::Arc;
use std::time::Instant;

use async_channel::{Receiver, Sender};
use tokio::sync::RwLock;
use tracing::{debug, error, info, instrument, warn};

use cortex_protocol::{ConversationId, Event, EventMsg, Op, Submission};

use crate::config::Config;
use crate::error::Result;
use crate::integrations::{
    ExperimentalIntegration, GhostIntegration, LspIntegration, MigrationIntegration,
    RatelimitIntegration, ResumeIntegration,
};

use super::{AgentConfig, AgentMetrics, ConversationTurn, TokenUsage, TurnStatus, UserInputItem};

/// The main Cortex Agent that orchestrates conversations.
pub struct CortexAgent {
    /// Configuration.
    #[allow(dead_code)]
    config: AgentConfig,
    /// Protocol config.
    protocol_config: Config,
    /// Conversation ID.
    conversation_id: ConversationId,
    /// Submission receiver.
    submission_rx: Receiver<Submission>,
    /// Event sender.
    event_tx: Sender<Event>,
    /// Current turn ID.
    turn_id: RwLock<u64>,
    /// Conversation history.
    turns: RwLock<Vec<ConversationTurn>>,
    /// Metrics.
    metrics: RwLock<AgentMetrics>,
    /// Running state.
    running: RwLock<bool>,

    // === NEW INTEGRATIONS ===
    /// Ghost commit integration for undo.
    ghost: RwLock<GhostIntegration>,
    /// LSP integration for diagnostics.
    lsp: Arc<LspIntegration>,
    /// Rate limit tracking.
    ratelimits: Arc<RatelimitIntegration>,
    /// Model migration warnings.
    migrations: MigrationIntegration,
    /// Experimental features.
    experimental: Arc<ExperimentalIntegration>,
    /// Session resume.
    resume: Arc<RwLock<ResumeIntegration>>,
}

impl CortexAgent {
    /// Create a new Cortex Agent.
    pub async fn new(
        config: Config,
        submission_rx: Receiver<Submission>,
        event_tx: Sender<Event>,
    ) -> Result<Self> {
        let mut metrics = AgentMetrics::default();
        metrics.start_time = Some(Instant::now());

        // Initialize integrations
        let experimental = Arc::new(ExperimentalIntegration::new());
        let ghost_enabled = experimental.is_enabled("ghost_commits").await;
        let lsp_enabled = experimental.is_enabled("lsp_diagnostics").await;

        let working_dir = config.cwd.clone();
        let sessions_dir = config.cortex_home.join("sessions");

        // Initialize LSP if enabled
        let lsp = Arc::new(LspIntegration::new(lsp_enabled));
        if lsp_enabled && let Err(e) = lsp.init(&working_dir).await {
            warn!("Failed to initialize LSP: {}", e);
        }

        // Initialize resume integration
        let resume = Arc::new(RwLock::new(ResumeIntegration::new(sessions_dir)));
        if let Err(e) = resume.write().await.init().await {
            warn!("Failed to initialize resume: {}", e);
        }

        Ok(Self {
            config: AgentConfig::default(),
            protocol_config: config,
            conversation_id: ConversationId::new(),
            submission_rx,
            event_tx,
            turn_id: RwLock::new(0),
            turns: RwLock::new(Vec::new()),
            metrics: RwLock::new(metrics),
            running: RwLock::new(false),
            // New integrations
            ghost: RwLock::new(GhostIntegration::new(ghost_enabled)),
            lsp,
            ratelimits: Arc::new(RatelimitIntegration::new()),
            migrations: MigrationIntegration::new(),
            experimental,
            resume,
        })
    }

    /// Initialize ghost commits for a repository.
    pub async fn init_ghost(&self, repo_root: &std::path::Path) -> Result<()> {
        let session_id = self.conversation_id.to_string();
        self.ghost
            .write()
            .await
            .init(repo_root, &session_id)
            .await
            .map_err(|e| crate::error::CortexError::Internal(e.to_string()))
    }

    /// Check model for deprecation warnings.
    pub fn check_model(&self, model: &str) -> Option<String> {
        self.migrations.format_warnings(model)
    }

    /// Get LSP diagnostics for a file.
    pub async fn get_diagnostics(
        &self,
        path: &std::path::Path,
    ) -> Vec<crate::cortex_lsp::Diagnostic> {
        self.lsp.get_diagnostics(path).await
    }

    /// Get rate limits.
    pub async fn get_rate_limits(&self) -> Option<crate::cortex_ratelimits::RateLimitInfo> {
        self.ratelimits.get_limits().await
    }

    /// Check if a feature is enabled.
    pub async fn is_feature_enabled(&self, feature: &str) -> bool {
        self.experimental.is_enabled(feature).await
    }

    /// Get experimental features integration.
    pub fn experimental(&self) -> &Arc<ExperimentalIntegration> {
        &self.experimental
    }

    /// Get resume integration.
    pub fn resume(&self) -> &Arc<RwLock<ResumeIntegration>> {
        &self.resume
    }

    /// Run the agent loop.
    #[instrument(skip(self))]
    pub async fn run(&mut self) -> Result<()> {
        *self.running.write().await = true;
        info!("Agent started");

        loop {
            let submission = match self.submission_rx.recv().await {
                Ok(s) => s,
                Err(_) => {
                    info!("Submission channel closed, shutting down");
                    break;
                }
            };

            if let Err(e) = self.handle_submission(submission).await {
                error!("Error handling submission: {}", e);
            }

            if !*self.running.read().await {
                break;
            }
        }

        Ok(())
    }

    /// Handle a submission.
    async fn handle_submission(&self, submission: Submission) -> Result<()> {
        match submission.op {
            Op::Shutdown => {
                self.handle_shutdown().await?;
            }
            Op::Interrupt => {
                self.handle_interrupt().await?;
            }
            Op::UserInput { items } => {
                self.handle_user_input(&submission.id, items).await?;
            }
            Op::UserTurn { items, .. } => {
                self.handle_user_turn(&submission.id, items).await?;
            }
            Op::Compact => {
                self.handle_compact().await?;
            }
            Op::Undo => {
                self.handle_undo().await?;
            }
            Op::ReloadMcpServers => {
                info!("Reloading MCP servers...");
            }
            Op::EnableMcpServer { name } => {
                info!("Enabling MCP server: {}...", name);
            }
            Op::DisableMcpServer { name } => {
                info!("Disabling MCP server: {}...", name);
            }
            _ => {
                debug!("Unhandled operation: {:?}", submission.op);
            }
        }
        Ok(())
    }

    /// Handle shutdown.
    async fn handle_shutdown(&self) -> Result<()> {
        info!("Shutdown requested");
        *self.running.write().await = false;
        self.emit(EventMsg::ShutdownComplete).await;
        Ok(())
    }

    /// Handle interrupt.
    async fn handle_interrupt(&self) -> Result<()> {
        info!("Interrupt requested");
        self.emit(EventMsg::TurnAborted(cortex_protocol::TurnAbortedEvent {
            reason: cortex_protocol::TurnAbortReason::Interrupted,
        }))
        .await;
        Ok(())
    }

    /// Handle user input.
    async fn handle_user_input(
        &self,
        _submission_id: &str,
        items: Vec<cortex_protocol::UserInput>,
    ) -> Result<()> {
        let turn_start = Instant::now();

        // Increment turn ID
        let turn_id = {
            let mut id = self.turn_id.write().await;
            *id += 1;
            *id
        };

        // Create ghost commit before turn (if enabled)
        let turn_id_str = turn_id.to_string();
        if let Err(e) = self
            .ghost
            .read()
            .await
            .snapshot_before_turn(&turn_id_str)
            .await
        {
            debug!("Ghost commit skipped: {}", e);
        }

        // Extract user message
        let user_message = items
            .iter()
            .filter_map(|item| match item {
                cortex_protocol::UserInput::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        info!(turn_id, "Starting turn");

        // Record turn in resume integration
        if let Err(e) = self.resume.write().await.record_turn(0).await {
            debug!("Failed to record turn in resume: {}", e);
        }

        // Emit task started
        self.emit(EventMsg::TaskStarted(cortex_protocol::TaskStartedEvent {
            model_context_window: self.protocol_config.model_context_window,
        }))
        .await;

        // Placeholder response
        let response = "Agent response placeholder".to_string();

        // Emit response
        self.emit(EventMsg::AgentMessage(cortex_protocol::AgentMessageEvent {
            id: None,
            parent_id: None,
            message: response.clone(),
            finish_reason: None,
        }))
        .await;

        // Record turn
        let turn_duration = turn_start.elapsed();
        let turn = ConversationTurn {
            id: turn_id,
            user_message: user_message.clone(),
            user_items: vec![UserInputItem::Text {
                content: user_message,
            }],
            assistant_response: Some(response.clone()),
            tool_calls: Vec::new(),
            token_usage: TokenUsage::default(),
            duration_ms: turn_duration.as_millis() as u64,
            status: TurnStatus::Completed,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };

        self.turns.write().await.push(turn);

        // Update metrics
        self.metrics.write().await.record_turn(turn_duration, 0);

        // Emit completion
        self.emit(EventMsg::TaskComplete(cortex_protocol::TaskCompleteEvent {
            last_agent_message: Some(response),
        }))
        .await;

        Ok(())
    }

    /// Handle user turn.
    async fn handle_user_turn(
        &self,
        submission_id: &str,
        items: Vec<cortex_protocol::UserInput>,
    ) -> Result<()> {
        self.handle_user_input(submission_id, items).await
    }

    /// Handle compaction.
    async fn handle_compact(&self) -> Result<()> {
        info!("Compacting context");
        self.metrics.write().await.record_compaction();
        Ok(())
    }

    /// Handle undo.
    async fn handle_undo(&self) -> Result<()> {
        info!("Undoing last turn");

        self.emit(EventMsg::UndoStarted(cortex_protocol::UndoStartedEvent {
            message: Some("Undoing last turn".to_string()),
        }))
        .await;

        // Try to restore from ghost commit first
        let ghost_restored = match self.ghost.read().await.undo_last().await {
            Ok(Some(commit)) => {
                info!("Restored to ghost commit: {}", commit.sha);
                true
            }
            Ok(None) => {
                debug!("No ghost commit available for undo");
                false
            }
            Err(e) => {
                warn!("Ghost undo failed: {}", e);
                false
            }
        };

        // Remove last turn from history
        self.turns.write().await.pop();

        let message = if ghost_restored {
            "Undo completed (files restored from ghost commit)"
        } else {
            "Undo completed (conversation only)"
        };

        self.emit(EventMsg::UndoCompleted(
            cortex_protocol::UndoCompletedEvent {
                success: true,
                message: Some(message.to_string()),
            },
        ))
        .await;

        Ok(())
    }

    /// Emit protocol event.
    async fn emit(&self, msg: EventMsg) {
        let turn_id = *self.turn_id.read().await;
        let event = Event {
            id: turn_id.to_string(),
            msg,
        };
        let _ = self.event_tx.send(event).await;
    }

    /// Get conversation ID.
    pub fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }

    /// Get configuration.
    pub fn config(&self) -> &Config {
        &self.protocol_config
    }

    /// Get metrics.
    pub async fn metrics(&self) -> AgentMetrics {
        self.metrics.read().await.clone()
    }

    /// Get turn history.
    pub async fn turns(&self) -> Vec<ConversationTurn> {
        self.turns.read().await.clone()
    }
}
