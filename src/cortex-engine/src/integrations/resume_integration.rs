//! Session resume integration for cortex-core.
//!
//! Connects cortex-resume to provide session persistence and resume.

use cortex_resume::{ResumePicker, SessionMeta, SessionStore, SessionSummary};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

/// Resume integration for session management.
pub struct ResumeIntegration {
    store: Arc<RwLock<SessionStore>>,
    current_session: Arc<RwLock<Option<SessionMeta>>>,
}

impl ResumeIntegration {
    /// Create a new resume integration.
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self {
            store: Arc::new(RwLock::new(SessionStore::new(sessions_dir))),
            current_session: Arc::new(RwLock::new(None)),
        }
    }

    /// Initialize the session store.
    pub async fn init(&self) -> anyhow::Result<()> {
        self.store.read().await.init().await?;
        Ok(())
    }

    /// Start a new session.
    pub async fn start_session(&self, cwd: PathBuf) -> anyhow::Result<SessionMeta> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let meta = SessionMeta::new(&session_id, cwd);

        self.store.write().await.save_session(&meta).await?;
        *self.current_session.write().await = Some(meta.clone());

        info!("Started new session: {}", session_id);
        Ok(meta)
    }

    /// Resume an existing session.
    pub async fn resume_session(&self, session_id: &str) -> anyhow::Result<SessionMeta> {
        let meta = self.store.write().await.get_session(session_id).await?;
        *self.current_session.write().await = Some(meta.clone());

        info!("Resumed session: {}", session_id);
        Ok(meta)
    }

    /// Resume the last session.
    pub async fn resume_last(&self) -> anyhow::Result<Option<SessionMeta>> {
        if let Some(meta) = self.store.write().await.get_last_session().await? {
            *self.current_session.write().await = Some(meta.clone());
            info!("Resumed last session: {}", meta.id);
            return Ok(Some(meta));
        }
        Ok(None)
    }

    /// Get current session.
    pub async fn current_session(&self) -> Option<SessionMeta> {
        self.current_session.read().await.clone()
    }

    /// Update current session after a turn.
    pub async fn record_turn(&self, tokens: u64) -> anyhow::Result<()> {
        let mut guard = self.current_session.write().await;
        if let Some(ref mut meta) = *guard {
            meta.increment_turn();
            meta.add_tokens(tokens);
            self.store.write().await.save_session(meta).await?;
        }
        Ok(())
    }

    /// Set session title.
    pub async fn set_title(&self, title: &str) -> anyhow::Result<()> {
        let mut guard = self.current_session.write().await;
        if let Some(ref mut meta) = *guard {
            meta.title = Some(title.to_string());
            self.store.write().await.save_session(meta).await?;
        }
        Ok(())
    }

    /// List all sessions.
    pub async fn list_sessions(
        &self,
        include_archived: bool,
    ) -> anyhow::Result<Vec<SessionSummary>> {
        self.store
            .write()
            .await
            .list_sessions(include_archived)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// Create a resume picker.
    pub async fn create_picker(&self) -> ResumePicker {
        let store = SessionStore::new(
            self.store
                .read()
                .await
                .get_session_dir("")
                .parent()
                .unwrap_or(&PathBuf::from(".")),
        );
        ResumePicker::new(store)
    }

    /// Archive current session.
    pub async fn archive_current(&self) -> anyhow::Result<()> {
        let guard = self.current_session.read().await;
        if let Some(ref meta) = *guard {
            self.store.write().await.archive_session(&meta.id).await?;
            info!("Archived session: {}", meta.id);
        }
        drop(guard);
        *self.current_session.write().await = None;
        Ok(())
    }

    /// Get session directory for current session.
    pub async fn current_session_dir(&self) -> Option<PathBuf> {
        let guard = self.current_session.read().await;
        guard
            .as_ref()
            .map(|m| self.store.blocking_read().get_session_dir(&m.id))
    }
}

#[allow(dead_code)]
impl ResumeIntegration {
    fn blocking_read(&self) -> tokio::sync::RwLockReadGuard<'_, SessionStore> {
        // This is a workaround for non-async contexts
        // In production, prefer async methods
        futures::executor::block_on(self.store.read())
    }
}
