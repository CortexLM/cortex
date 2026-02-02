use uuid::Uuid;

/// Summary of a session for the sidebar
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub id: Uuid,
    pub title: String,
    pub last_message: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub message_count: usize,
}

impl SessionSummary {
    /// Create a new session summary with minimal info
    pub fn new(id: Uuid, title: String) -> Self {
        Self {
            id,
            title,
            last_message: String::new(),
            timestamp: chrono::Utc::now(),
            message_count: 0,
        }
    }

    /// Get a human-readable relative time string
    pub fn relative_time(&self) -> String {
        let elapsed = chrono::Utc::now().signed_duration_since(self.timestamp);
        let secs = elapsed.num_seconds();

        if secs < 60 {
            "just now".to_string()
        } else if secs < 3600 {
            format!("{}m ago", secs / 60)
        } else if secs < 86400 {
            format!("{}h ago", secs / 3600)
        } else {
            format!("{}d ago", secs / 86400)
        }
    }

    pub fn with_message_count(mut self, count: usize) -> Self {
        self.message_count = count;
        self
    }

    pub fn with_timestamp(mut self, timestamp: chrono::DateTime<chrono::Utc>) -> Self {
        self.timestamp = timestamp;
        self
    }

    /// Sets the last message preview.
    pub fn with_last_message(mut self, message: impl Into<String>) -> Self {
        self.last_message = message.into();
        self
    }
}

/// Currently active modal dialog
#[derive(Debug, Clone)]
pub enum ActiveModal {
    Form(crate::widgets::FormState),
    // ProviderPicker - removed: provider is now always "cortex"
    ModelPicker,
    CommandPalette,
    Export,
    Fork,
    ThemePicker,
}
