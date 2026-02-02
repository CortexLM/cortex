//! Event system for plugin communication.
//!
//! The event bus allows plugins to subscribe to and publish system events.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

/// System events that plugins can subscribe to.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    // ========== Session Events ==========
    /// Session started
    SessionStarted {
        session_id: String,
        agent: Option<String>,
        model: Option<String>,
    },

    /// Session ended
    SessionEnded {
        session_id: String,
        duration_ms: u64,
    },

    /// Session resumed
    SessionResumed {
        session_id: String,
        message_count: usize,
    },

    // ========== Tool Events ==========
    /// Tool execution started
    ToolExecutionStarted {
        session_id: String,
        tool_name: String,
        call_id: String,
    },

    /// Tool execution completed
    ToolExecutionCompleted {
        session_id: String,
        tool_name: String,
        call_id: String,
        success: bool,
        duration_ms: u64,
    },

    /// Tool was aborted
    ToolExecutionAborted {
        session_id: String,
        tool_name: String,
        call_id: String,
        reason: String,
    },

    // ========== Chat/AI Events ==========
    /// Chat message received
    ChatMessageReceived {
        session_id: String,
        message_id: String,
        role: String,
    },

    /// Chat message sent
    ChatMessageSent {
        session_id: String,
        message_id: String,
    },

    /// AI response started
    AiResponseStarted {
        session_id: String,
        request_id: String,
        model: String,
    },

    /// AI response streaming chunk
    AiResponseChunk {
        session_id: String,
        request_id: String,
        chunk_index: usize,
    },

    /// AI response completed
    AiResponseCompleted {
        session_id: String,
        request_id: String,
        model: String,
        duration_ms: u64,
        tokens_used: Option<u32>,
    },

    /// AI response error
    AiResponseError {
        session_id: String,
        request_id: String,
        error: String,
    },

    // ========== File Events ==========
    /// File edited
    FileEdited {
        session_id: String,
        file_path: String,
        change_type: FileChangeType,
    },

    /// File created
    FileCreated {
        session_id: String,
        file_path: String,
    },

    /// File deleted
    FileDeleted {
        session_id: String,
        file_path: String,
    },

    /// File renamed
    FileRenamed {
        session_id: String,
        old_path: String,
        new_path: String,
    },

    // ========== Command Events ==========
    /// Command execution started
    CommandStarted {
        session_id: String,
        command: String,
        args: Vec<String>,
    },

    /// Command execution completed
    CommandCompleted {
        session_id: String,
        command: String,
        success: bool,
        duration_ms: u64,
    },

    // ========== Config Events ==========
    /// Model changed
    ModelChanged {
        session_id: String,
        old_model: Option<String>,
        new_model: String,
    },

    /// Configuration changed
    ConfigChanged {
        key: String,
        old_value: Option<serde_json::Value>,
        new_value: serde_json::Value,
    },

    /// Agent changed
    AgentChanged {
        session_id: String,
        old_agent: Option<String>,
        new_agent: String,
    },

    // ========== Workspace Events ==========
    /// Working directory changed
    WorkspaceChanged {
        session_id: String,
        old_cwd: Option<String>,
        new_cwd: String,
    },

    /// Project detected
    ProjectDetected {
        session_id: String,
        project_type: String,
        root_path: String,
    },

    // ========== Plugin Events ==========
    /// Plugin loaded
    PluginLoaded { plugin_id: String },

    /// Plugin unloaded
    PluginUnloaded { plugin_id: String },

    /// Plugin error
    PluginError { plugin_id: String, error: String },

    /// Plugin initialized
    PluginInitialized { plugin_id: String },

    /// Plugin disabled
    PluginDisabled { plugin_id: String },

    /// Plugin enabled
    PluginEnabled { plugin_id: String },

    // ========== Error Events ==========
    /// Error occurred
    ErrorOccurred {
        session_id: String,
        error_type: String,
        message: String,
        recoverable: bool,
    },

    // ========== Clipboard Events ==========
    /// Content copied to clipboard
    ClipboardCopy {
        session_id: String,
        content_length: usize,
    },

    /// Content pasted from clipboard
    ClipboardPaste {
        session_id: String,
        content_length: usize,
    },

    // ========== Focus Events ==========
    /// Application gained focus
    FocusGained { session_id: String },

    /// Application lost focus
    FocusLost { session_id: String },

    // ========== Permission Events ==========
    /// Permission requested
    PermissionRequested {
        session_id: String,
        permission: String,
        resource: String,
    },

    /// Permission granted
    PermissionGranted {
        session_id: String,
        permission: String,
        resource: String,
    },

    /// Permission denied
    PermissionDenied {
        session_id: String,
        permission: String,
        resource: String,
    },

    // ========== Custom Events ==========
    /// Custom event from a plugin
    Custom {
        plugin_id: String,
        event_type: String,
        data: serde_json::Value,
    },

    /// Inter-plugin message
    PluginMessage {
        source_plugin: String,
        target_plugin: Option<String>,
        message_type: String,
        payload: serde_json::Value,
    },
}

/// File change types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeType {
    Created,
    Modified,
    Deleted,
    Renamed,
}

/// Event handler trait for plugins.
#[async_trait]
pub trait EventHandler: Send + Sync {
    /// Handle an event.
    async fn handle(&self, event: &Event) -> crate::Result<()>;

    /// Get the event types this handler is interested in.
    fn event_types(&self) -> Vec<EventType>;
}

/// Event type filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventType {
    // Session events
    SessionStarted,
    SessionEnded,
    SessionResumed,
    // Tool events
    ToolExecutionStarted,
    ToolExecutionCompleted,
    ToolExecutionAborted,
    // Chat/AI events
    ChatMessageReceived,
    ChatMessageSent,
    AiResponseStarted,
    AiResponseChunk,
    AiResponseCompleted,
    AiResponseError,
    // File events
    FileEdited,
    FileCreated,
    FileDeleted,
    FileRenamed,
    // Command events
    CommandStarted,
    CommandCompleted,
    // Config events
    ModelChanged,
    ConfigChanged,
    AgentChanged,
    // Workspace events
    WorkspaceChanged,
    ProjectDetected,
    // Plugin events
    PluginLoaded,
    PluginUnloaded,
    PluginError,
    PluginInitialized,
    PluginDisabled,
    PluginEnabled,
    // Error events
    ErrorOccurred,
    // Clipboard events
    ClipboardCopy,
    ClipboardPaste,
    // Focus events
    FocusGained,
    FocusLost,
    // Permission events
    PermissionRequested,
    PermissionGranted,
    PermissionDenied,
    // Custom events
    Custom,
    PluginMessage,
    // Wildcard
    All,
}

impl From<&Event> for EventType {
    fn from(event: &Event) -> Self {
        match event {
            // Session events
            Event::SessionStarted { .. } => EventType::SessionStarted,
            Event::SessionEnded { .. } => EventType::SessionEnded,
            Event::SessionResumed { .. } => EventType::SessionResumed,
            // Tool events
            Event::ToolExecutionStarted { .. } => EventType::ToolExecutionStarted,
            Event::ToolExecutionCompleted { .. } => EventType::ToolExecutionCompleted,
            Event::ToolExecutionAborted { .. } => EventType::ToolExecutionAborted,
            // Chat/AI events
            Event::ChatMessageReceived { .. } => EventType::ChatMessageReceived,
            Event::ChatMessageSent { .. } => EventType::ChatMessageSent,
            Event::AiResponseStarted { .. } => EventType::AiResponseStarted,
            Event::AiResponseChunk { .. } => EventType::AiResponseChunk,
            Event::AiResponseCompleted { .. } => EventType::AiResponseCompleted,
            Event::AiResponseError { .. } => EventType::AiResponseError,
            // File events
            Event::FileEdited { .. } => EventType::FileEdited,
            Event::FileCreated { .. } => EventType::FileCreated,
            Event::FileDeleted { .. } => EventType::FileDeleted,
            Event::FileRenamed { .. } => EventType::FileRenamed,
            // Command events
            Event::CommandStarted { .. } => EventType::CommandStarted,
            Event::CommandCompleted { .. } => EventType::CommandCompleted,
            // Config events
            Event::ModelChanged { .. } => EventType::ModelChanged,
            Event::ConfigChanged { .. } => EventType::ConfigChanged,
            Event::AgentChanged { .. } => EventType::AgentChanged,
            // Workspace events
            Event::WorkspaceChanged { .. } => EventType::WorkspaceChanged,
            Event::ProjectDetected { .. } => EventType::ProjectDetected,
            // Plugin events
            Event::PluginLoaded { .. } => EventType::PluginLoaded,
            Event::PluginUnloaded { .. } => EventType::PluginUnloaded,
            Event::PluginError { .. } => EventType::PluginError,
            Event::PluginInitialized { .. } => EventType::PluginInitialized,
            Event::PluginDisabled { .. } => EventType::PluginDisabled,
            Event::PluginEnabled { .. } => EventType::PluginEnabled,
            // Error events
            Event::ErrorOccurred { .. } => EventType::ErrorOccurred,
            // Clipboard events
            Event::ClipboardCopy { .. } => EventType::ClipboardCopy,
            Event::ClipboardPaste { .. } => EventType::ClipboardPaste,
            // Focus events
            Event::FocusGained { .. } => EventType::FocusGained,
            Event::FocusLost { .. } => EventType::FocusLost,
            // Permission events
            Event::PermissionRequested { .. } => EventType::PermissionRequested,
            Event::PermissionGranted { .. } => EventType::PermissionGranted,
            Event::PermissionDenied { .. } => EventType::PermissionDenied,
            // Custom events
            Event::Custom { .. } => EventType::Custom,
            Event::PluginMessage { .. } => EventType::PluginMessage,
        }
    }
}

/// Event subscription handle.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EventSubscription {
    id: Uuid,
    plugin_id: String,
}

impl EventSubscription {
    /// Create a new subscription.
    pub fn new(plugin_id: &str) -> Self {
        Self {
            id: Uuid::new_v4(),
            plugin_id: plugin_id.to_string(),
        }
    }

    /// Get the subscription ID.
    pub fn id(&self) -> Uuid {
        self.id
    }

    /// Get the plugin ID.
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }
}

/// Handler entry with its filter.
type HandlerEntry = (Arc<dyn EventHandler>, Vec<EventType>);

/// Event bus for publishing and subscribing to events.
pub struct EventBus {
    handlers: RwLock<HashMap<Uuid, HandlerEntry>>,
    subscriptions: RwLock<HashMap<Uuid, EventSubscription>>,
}

impl EventBus {
    /// Create a new event bus.
    pub fn new() -> Self {
        Self {
            handlers: RwLock::new(HashMap::new()),
            subscriptions: RwLock::new(HashMap::new()),
        }
    }

    /// Subscribe to events.
    pub async fn subscribe(
        &self,
        plugin_id: &str,
        handler: Arc<dyn EventHandler>,
    ) -> EventSubscription {
        let subscription = EventSubscription::new(plugin_id);
        let event_types = handler.event_types();

        let mut handlers = self.handlers.write().await;
        handlers.insert(subscription.id, (handler, event_types));

        let mut subscriptions = self.subscriptions.write().await;
        subscriptions.insert(subscription.id, subscription.clone());

        subscription
    }

    /// Unsubscribe from events.
    pub async fn unsubscribe(&self, subscription: &EventSubscription) {
        let mut handlers = self.handlers.write().await;
        handlers.remove(&subscription.id);

        let mut subscriptions = self.subscriptions.write().await;
        subscriptions.remove(&subscription.id);
    }

    /// Unsubscribe all handlers for a plugin.
    pub async fn unsubscribe_plugin(&self, plugin_id: &str) {
        let subscriptions = self.subscriptions.read().await;
        let to_remove: Vec<_> = subscriptions
            .values()
            .filter(|s| s.plugin_id == plugin_id)
            .map(|s| s.id)
            .collect();
        drop(subscriptions);

        let mut handlers = self.handlers.write().await;
        let mut subscriptions = self.subscriptions.write().await;

        for id in to_remove {
            handlers.remove(&id);
            subscriptions.remove(&id);
        }
    }

    /// Publish an event to all subscribers.
    pub async fn publish(&self, event: Event) {
        let event_type = EventType::from(&event);
        let handlers = self.handlers.read().await;

        for (handler, types) in handlers.values() {
            if types.contains(&EventType::All) || types.contains(&event_type) {
                // Fire and forget - don't block on handler errors
                let handler = handler.clone();
                let event = event.clone();
                tokio::spawn(async move {
                    if let Err(e) = handler.handle(&event).await {
                        tracing::warn!("Event handler error: {}", e);
                    }
                });
            }
        }
    }

    /// Get the number of active subscriptions.
    pub async fn subscription_count(&self) -> usize {
        self.subscriptions.read().await.len()
    }

    /// Get subscriptions for a plugin.
    pub async fn plugin_subscriptions(&self, plugin_id: &str) -> Vec<EventSubscription> {
        self.subscriptions
            .read()
            .await
            .values()
            .filter(|s| s.plugin_id == plugin_id)
            .cloned()
            .collect()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct TestHandler {
        count: Arc<AtomicU32>,
        event_types: Vec<EventType>,
    }

    #[async_trait]
    impl EventHandler for TestHandler {
        async fn handle(&self, _event: &Event) -> crate::Result<()> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn event_types(&self) -> Vec<EventType> {
            self.event_types.clone()
        }
    }

    #[tokio::test]
    async fn test_event_bus_subscribe_publish() {
        let bus = EventBus::new();
        let count = Arc::new(AtomicU32::new(0));

        let handler = Arc::new(TestHandler {
            count: count.clone(),
            event_types: vec![EventType::SessionStarted],
        });

        let _sub = bus.subscribe("test-plugin", handler).await;

        bus.publish(Event::SessionStarted {
            session_id: "test".to_string(),
            agent: None,
            model: None,
        })
        .await;

        // Give async handler time to run
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_event_bus_unsubscribe() {
        let bus = EventBus::new();
        let count = Arc::new(AtomicU32::new(0));

        let handler = Arc::new(TestHandler {
            count: count.clone(),
            event_types: vec![EventType::All],
        });

        let sub = bus.subscribe("test-plugin", handler).await;
        assert_eq!(bus.subscription_count().await, 1);

        bus.unsubscribe(&sub).await;
        assert_eq!(bus.subscription_count().await, 0);
    }

    #[tokio::test]
    async fn test_event_bus_filter_types() {
        let bus = EventBus::new();
        let count = Arc::new(AtomicU32::new(0));

        let handler = Arc::new(TestHandler {
            count: count.clone(),
            event_types: vec![EventType::SessionStarted],
        });

        let _sub = bus.subscribe("test-plugin", handler).await;

        // This event should not trigger the handler
        bus.publish(Event::SessionEnded {
            session_id: "test".to_string(),
            duration_ms: 1000,
        })
        .await;

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }
}
