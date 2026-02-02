//! Ghost commit integration for cortex-core.
//!
//! Connects cortex-ghost to provide automatic undo capability.

use cortex_ghost::{GhostCommit, GhostCommitManager, GhostConfig};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Ghost commit integration for automatic undo.
pub struct GhostIntegration {
    manager: Arc<RwLock<Option<GhostCommitManager>>>,
    config: GhostConfig,
    enabled: bool,
    session_id: Option<String>,
    repo_root: Option<PathBuf>,
}

impl GhostIntegration {
    /// Create a new ghost integration.
    pub fn new(enabled: bool) -> Self {
        Self {
            manager: Arc::new(RwLock::new(None)),
            config: GhostConfig::default(),
            enabled,
            session_id: None,
            repo_root: None,
        }
    }

    /// Initialize ghost commits for a repository.
    pub async fn init(&mut self, repo_root: &Path, session_id: &str) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }

        self.session_id = Some(session_id.to_string());
        self.repo_root = Some(repo_root.to_path_buf());

        let manager = GhostCommitManager::new(repo_root.to_path_buf(), self.config.clone());

        *self.manager.write().await = Some(manager);
        info!("Ghost commits initialized for session {}", session_id);
        Ok(())
    }

    /// Create a ghost commit before a turn.
    pub async fn snapshot_before_turn(&self, turn_id: &str) -> anyhow::Result<Option<String>> {
        if !self.enabled {
            return Ok(None);
        }

        let mut guard = self.manager.write().await;
        if let Some(ref mut manager) = *guard {
            let session_id = self.session_id.as_deref().unwrap_or("unknown");
            match manager
                .create_ghost_commit(turn_id, session_id, None, None)
                .await
            {
                Ok((commit, _report)) => {
                    debug!("Created ghost commit {} for turn {}", commit.sha, turn_id);
                    Ok(Some(commit.sha))
                }
                Err(e) => {
                    warn!("Failed to create ghost commit: {}", e);
                    Ok(None)
                }
            }
        } else {
            Ok(None)
        }
    }

    /// Undo to a previous turn.
    pub async fn undo_to_turn(&self, turn_id: &str) -> anyhow::Result<bool> {
        let guard = self.manager.read().await;
        if let Some(ref manager) = *guard {
            let session_id = self.session_id.as_deref().unwrap_or("unknown");
            let commits = manager.get_session_commits(session_id);

            if let Some(commit) = commits.iter().find(|c| c.turn_id == turn_id) {
                let repo_root = self.repo_root.clone().unwrap_or_default();
                cortex_ghost::restore::restore_ghost_commit(&repo_root, commit, Default::default())
                    .await?;
                info!("Restored to turn {}", turn_id);
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Undo the last turn.
    pub async fn undo_last(&self) -> anyhow::Result<Option<GhostCommit>> {
        let guard = self.manager.read().await;
        if let Some(ref manager) = *guard {
            let session_id = self.session_id.as_deref().unwrap_or("unknown");
            let commits = manager.get_session_commits(session_id);

            if let Some(commit) = commits.last() {
                let repo_root = self.repo_root.clone().unwrap_or_default();
                cortex_ghost::restore::restore_ghost_commit(&repo_root, commit, Default::default())
                    .await?;
                info!("Undid last turn, restored to {}", commit.sha);
                return Ok(Some((*commit).clone()));
            }
        }
        Ok(None)
    }

    /// Get history of ghost commits for this session.
    pub fn get_history(&self) -> Vec<GhostCommit> {
        if let Some(ref manager) = *futures::executor::block_on(self.manager.read()) {
            let session_id = self.session_id.as_deref().unwrap_or("unknown");
            manager
                .get_session_commits(session_id)
                .into_iter()
                .cloned()
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Check if ghost commits are enabled and available.
    pub async fn is_available(&self) -> bool {
        self.enabled && self.manager.read().await.is_some()
    }

    /// Set enabled state.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
}

impl Default for GhostIntegration {
    fn default() -> Self {
        Self::new(false)
    }
}
