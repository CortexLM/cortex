//! Session lifecycle - creation, resumption, forking, and listing.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use async_channel::unbounded;
use chrono::Utc;
use tracing::warn;

use cortex_protocol::{
    AgentMessageEvent, ConversationId, Event, EventMsg, TokenUsage, UserMessageEvent,
};

use crate::client::{Message, create_client};
use crate::config::Config;
use crate::error::Result;
use crate::rollout::reader::{RolloutItem, get_events, get_session_meta};
use crate::rollout::recorder::SessionMeta;
use crate::rollout::{RolloutRecorder, SESSIONS_SUBDIR, get_rollout_path, read_rollout};
use crate::tools::ToolRouter;

use super::Session;
use super::prompt::build_system_prompt;
use super::types::{SessionHandle, SessionInfo, TokenCounter};

impl Session {
    /// Create a new session with channels.
    pub fn new(config: Config) -> Result<(Self, SessionHandle)> {
        let (submission_tx, submission_rx) = unbounded();
        let (event_tx, event_rx) = unbounded();

        let conversation_id = ConversationId::new();
        let cancelled = Arc::new(AtomicBool::new(false));

        // Get API key using centralized auth module
        let api_key = crate::auth_token::get_auth_token(None)
            .map_err(|e| anyhow::anyhow!("Authentication required: {}", e))?;

        let client = create_client(
            &config.model_provider_id,
            &config.model,
            &api_key,
            Some(config.model_provider.base_url.as_str()),
        )?;

        let mut tool_router = ToolRouter::new();

        // Initialize rollout recorder
        let mut recorder = RolloutRecorder::new(&config.cortex_home, conversation_id)?;
        recorder.init()?;

        // Record session metadata
        let meta = SessionMeta {
            id: conversation_id,
            parent_id: None,
            fork_point: None,
            timestamp: Utc::now().to_rfc3339(),
            cwd: config.cwd.clone(),
            model: config.model.clone(),
            cli_version: env!("CARGO_PKG_VERSION").to_string(),
            instructions: config.user_instructions.clone(),
        };
        recorder.record_meta(&meta)?;

        // Initialize with system prompt
        let mut messages = Vec::new();
        messages.push(Message::system(build_system_prompt(&config)));

        // Initialize snapshot manager
        let snapshot_dir = config
            .cortex_home
            .join("snapshots")
            .join(conversation_id.to_string());
        let snapshot_manager =
            Arc::new(crate::tasks::snapshot::SnapshotManager::new(50).with_storage(snapshot_dir));

        // Initialize LSP integration
        let lsp = Arc::new(crate::integrations::LspIntegration::new(true));
        let lsp_clone = lsp.clone();
        let cwd_clone = config.cwd.clone();
        tokio::spawn(async move {
            if let Err(e) = lsp_clone.init(&cwd_clone).await {
                warn!("Failed to initialize LSP in session: {}", e);
            }
        });

        tool_router.set_lsp(lsp.clone());

        let session = Self {
            config,
            conversation_id,
            client,
            tool_router,
            messages,
            submission_rx,
            event_tx,
            turn_id: 0,
            total_usage: TokenUsage::default(),
            token_counter: Arc::new(TokenCounter::default()),
            running: true,
            recorder: Some(recorder),
            pending_approvals: std::collections::HashMap::new(),
            cancelled: cancelled.clone(),
            snapshot_manager,
            undo_history: crate::tasks::undo::UndoHistory::new(50),
            redo_history: crate::tasks::undo::RedoHistory::new(50),
            current_undo_actions: Vec::new(),
            share_service: crate::share_service::ShareService::new(),
            lsp,
        };

        let handle = SessionHandle {
            submission_tx,
            event_rx,
            conversation_id: session.conversation_id,
            cancelled,
        };

        Ok((session, handle))
    }

    /// Resume a session from a rollout file.
    pub fn resume(
        config: Config,
        conversation_id: ConversationId,
    ) -> Result<(Self, SessionHandle)> {
        let (submission_tx, submission_rx) = unbounded();
        let (event_tx, event_rx) = unbounded();

        let rollout_path = get_rollout_path(&config.cortex_home, &conversation_id);
        let entries = read_rollout(&rollout_path)?;

        // Get API key using centralized auth module
        let api_key = crate::auth_token::get_auth_token(None)
            .map_err(|e| anyhow::anyhow!("Authentication required: {}", e))?;

        let client = create_client(
            &config.model_provider_id,
            &config.model,
            &api_key,
            Some(config.model_provider.base_url.as_str()),
        )?;

        let mut tool_router = ToolRouter::new();

        // Rebuild messages from events
        let mut messages = Vec::new();
        messages.push(Message::system(build_system_prompt(&config)));

        let events = get_events(&entries);
        for event_msg in events {
            match event_msg {
                EventMsg::UserMessage(e) => {
                    messages.push(Message::user(&e.message));
                }
                EventMsg::AgentMessage(e) => {
                    messages.push(Message::assistant(&e.message));
                }
                EventMsg::UndoCompleted(e) => {
                    if e.success {
                        // Replicate handle_undo logic
                        while let Some(msg) = messages.last() {
                            if matches!(msg.role, crate::client::MessageRole::User) {
                                break;
                            }
                            messages.pop();
                        }
                        if let Some(msg) = messages.last()
                            && matches!(msg.role, crate::client::MessageRole::User)
                        {
                            messages.pop();
                        }
                    }
                }
                _ => {}
            }
        }

        // Initialize rollout recorder
        let mut recorder = RolloutRecorder::new(&config.cortex_home, conversation_id)?;
        recorder.init()?;

        // Initialize LSP integration
        let lsp = Arc::new(crate::integrations::LspIntegration::new(true));
        let lsp_clone = lsp.clone();
        let cwd_clone = config.cwd.clone();
        tokio::spawn(async move {
            if let Err(e) = lsp_clone.init(&cwd_clone).await {
                warn!("Failed to initialize LSP in resumed session: {}", e);
            }
        });

        tool_router.set_lsp(lsp.clone());

        let cancelled = Arc::new(AtomicBool::new(false));

        let session = Self {
            config,
            conversation_id,
            client,
            tool_router,
            messages,
            submission_rx,
            event_tx,
            turn_id: 0,
            total_usage: TokenUsage::default(),
            token_counter: Arc::new(TokenCounter::default()),
            running: true,
            recorder: Some(recorder),
            pending_approvals: std::collections::HashMap::new(),
            cancelled: cancelled.clone(),
            snapshot_manager: Arc::new(crate::tasks::snapshot::SnapshotManager::new(50)),
            undo_history: crate::tasks::undo::UndoHistory::new(50),
            redo_history: crate::tasks::undo::RedoHistory::new(50),
            current_undo_actions: Vec::new(),
            share_service: crate::share_service::ShareService::new(),
            lsp,
        };

        let handle = SessionHandle {
            submission_tx,
            event_rx,
            conversation_id,
            cancelled,
        };

        Ok((session, handle))
    }

    /// Fork a session from an existing conversation.
    pub fn fork(
        config: Config,
        original_conversation_id: ConversationId,
        message_index: usize,
    ) -> Result<(Self, SessionHandle)> {
        let (submission_tx, submission_rx) = unbounded();
        let (event_tx, event_rx) = unbounded();

        let new_conversation_id = ConversationId::new();

        // Get API key using centralized auth module
        let api_key = crate::auth_token::get_auth_token(None)
            .map_err(|e| anyhow::anyhow!("Authentication required: {}", e))?;

        let client = create_client(
            &config.model_provider_id,
            &config.model,
            &api_key,
            Some(config.model_provider.base_url.as_str()),
        )?;

        let mut tool_router = ToolRouter::new();

        // Rebuild messages from original rollout up to index
        let rollout_path = get_rollout_path(&config.cortex_home, &original_conversation_id);
        let entries = read_rollout(&rollout_path)?;

        let mut messages = Vec::new();
        messages.push(Message::system(build_system_prompt(&config)));

        let events = get_events(&entries);

        // Count messages (User + Assistant) to decide where to stop
        let mut count = 0;
        for event_msg in events {
            let mut added = false;
            match event_msg {
                EventMsg::UserMessage(e) => {
                    messages.push(Message::user(&e.message));
                    added = true;
                }
                EventMsg::AgentMessage(e) => {
                    messages.push(Message::assistant(&e.message));
                    added = true;
                }
                _ => {}
            }
            if added {
                if count >= message_index {
                    break;
                }
                count += 1;
            }
        }

        // Initialize new rollout recorder
        let mut recorder = RolloutRecorder::new(&config.cortex_home, new_conversation_id)?;
        recorder.init()?;

        // Record session metadata
        let meta = SessionMeta {
            id: new_conversation_id,
            parent_id: Some(original_conversation_id),
            fork_point: Some(message_index.to_string()),
            timestamp: Utc::now().to_rfc3339(),
            cwd: config.cwd.clone(),
            model: config.model.clone(),
            cli_version: env!("CARGO_PKG_VERSION").to_string(),
            instructions: config.user_instructions.clone(),
        };
        recorder.record_meta(&meta)?;

        // Record the copied events to the new rollout
        let mut user_msg_count = 0;
        for msg in &messages {
            match msg.role {
                crate::client::MessageRole::User => {
                    user_msg_count += 1;
                    if let Some(text) = msg.content.as_text() {
                        recorder.record_event(&Event {
                            id: "0".to_string(),
                            msg: EventMsg::UserMessage(UserMessageEvent {
                                id: None,
                                parent_id: None,
                                message: text.to_string(),
                                images: None,
                            }),
                        })?;
                    }
                }
                crate::client::MessageRole::Assistant => {
                    if let Some(text) = msg.content.as_text() {
                        recorder.record_event(&Event {
                            id: "0".to_string(),
                            msg: EventMsg::AgentMessage(AgentMessageEvent {
                                id: None,
                                parent_id: None,
                                message: text.to_string(),
                                finish_reason: None,
                            }),
                        })?;
                    }
                }
                _ => {}
            }
        }

        // Initialize snapshot manager
        let snapshot_dir = config
            .cortex_home
            .join("snapshots")
            .join(new_conversation_id.to_string());
        let snapshot_manager =
            Arc::new(crate::tasks::snapshot::SnapshotManager::new(50).with_storage(snapshot_dir));

        // Initialize LSP integration
        let lsp = Arc::new(crate::integrations::LspIntegration::new(true));
        let lsp_clone = lsp.clone();
        let cwd_clone = config.cwd.clone();
        tokio::spawn(async move {
            if let Err(e) = lsp_clone.init(&cwd_clone).await {
                warn!("Failed to initialize LSP in forked session: {}", e);
            }
        });

        tool_router.set_lsp(lsp.clone());

        let cancelled = Arc::new(AtomicBool::new(false));

        let session = Self {
            config,
            conversation_id: new_conversation_id,
            client,
            tool_router,
            messages,
            submission_rx,
            event_tx,
            turn_id: user_msg_count as u64,
            total_usage: TokenUsage::default(),
            token_counter: Arc::new(TokenCounter::default()),
            running: true,
            recorder: Some(recorder),
            pending_approvals: std::collections::HashMap::new(),
            cancelled: cancelled.clone(),
            snapshot_manager,
            undo_history: crate::tasks::undo::UndoHistory::new(50),
            redo_history: crate::tasks::undo::RedoHistory::new(50),
            current_undo_actions: Vec::new(),
            share_service: crate::share_service::ShareService::new(),
            lsp,
        };

        let handle = SessionHandle {
            submission_tx,
            event_rx,
            conversation_id: session.conversation_id,
            cancelled,
        };

        Ok((session, handle))
    }
}

/// List available sessions.
pub fn list_sessions(cortex_home: &PathBuf) -> Result<Vec<SessionInfo>> {
    let sessions_dir = cortex_home.join(SESSIONS_SUBDIR);

    if !sessions_dir.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();

    for entry in std::fs::read_dir(&sessions_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.extension().is_some_and(|e| e == "jsonl")
            && let Ok(entries) = read_rollout(&path)
            && let Some(meta) = get_session_meta(&entries)
        {
            let cwd = PathBuf::from(&meta.cwd);
            let git_branch = get_git_branch_for_dir(&cwd);
            sessions.push(SessionInfo {
                id: meta.id.clone(),
                timestamp: meta.timestamp.clone(),
                model: meta.model.clone(),
                cwd,
                message_count: entries
                    .iter()
                    .filter(|e| matches!(e.item, RolloutItem::EventMsg(EventMsg::UserMessage(_))))
                    .count(),
                git_branch,
            });
        }
    }

    // Sort by timestamp (newest first)
    sessions.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    Ok(sessions)
}

/// Get git branch for a directory.
fn get_git_branch_for_dir(dir: &PathBuf) -> Option<String> {
    if !dir.exists() {
        return None;
    }
    std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(dir)
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
            } else {
                None
            }
        })
}
