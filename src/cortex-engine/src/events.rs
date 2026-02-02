//! Event system.
//!
//! Provides an event bus for communication between components.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, broadcast};

/// Event priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum Priority {
    /// Low priority.
    Low,
    /// Normal priority.
    #[default]
    Normal,
    /// High priority.
    High,
    /// Critical priority.
    Critical,
}

/// Event category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum EventCategory {
    /// System events.
    #[default]
    System,
    /// User events.
    User,
    /// Agent events.
    Agent,
    /// Tool events.
    Tool,
    /// Model events.
    Model,
    /// Error events.
    Error,
    /// Custom events.
    Custom,
}

/// Base event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Event ID.
    pub id: String,
    /// Event type.
    pub event_type: String,
    /// Category.
    pub category: EventCategory,
    /// Priority.
    pub priority: Priority,
    /// Timestamp.
    pub timestamp: u64,
    /// Source.
    pub source: Option<String>,
    /// Payload.
    pub payload: serde_json::Value,
    /// Metadata.
    pub metadata: HashMap<String, String>,
}

impl Event {
    /// Create a new event.
    pub fn new(event_type: impl Into<String>) -> Self {
        Self {
            id: generate_event_id(),
            event_type: event_type.into(),
            category: EventCategory::default(),
            priority: Priority::default(),
            timestamp: timestamp_now(),
            source: None,
            payload: serde_json::Value::Null,
            metadata: HashMap::new(),
        }
    }

    /// Set category.
    pub fn category(mut self, category: EventCategory) -> Self {
        self.category = category;
        self
    }

    /// Set priority.
    pub fn priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self
    }

    /// Set source.
    pub fn source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Set payload.
    pub fn payload<T: Serialize>(mut self, payload: T) -> Self {
        self.payload = serde_json::to_value(payload).unwrap_or(serde_json::Value::Null);
        self
    }

    /// Add metadata.
    pub fn meta(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Get payload as type.
    pub fn get_payload<T: for<'de> Deserialize<'de>>(&self) -> Option<T> {
        serde_json::from_value(self.payload.clone()).ok()
    }
}

/// Event listener trait.
#[async_trait::async_trait]
pub trait EventListener: Send + Sync {
    /// Called when an event is received.
    async fn on_event(&self, event: &Event);

    /// Get the event types this listener handles.
    fn handles(&self) -> Option<Vec<String>> {
        None // Handles all events
    }

    /// Get the categories this listener handles.
    fn categories(&self) -> Option<Vec<EventCategory>> {
        None // Handles all categories
    }
}

/// Event bus.
pub struct EventBus {
    /// Broadcast sender.
    sender: broadcast::Sender<Event>,
    /// Listeners.
    listeners: RwLock<Vec<Arc<dyn EventListener>>>,
    /// Event history.
    history: RwLock<Vec<Event>>,
    /// History limit.
    history_limit: usize,
}

impl EventBus {
    /// Create a new event bus.
    pub fn new(buffer_size: usize) -> Self {
        let (sender, _) = broadcast::channel(buffer_size);
        Self {
            sender,
            listeners: RwLock::new(Vec::new()),
            history: RwLock::new(Vec::new()),
            history_limit: 1000,
        }
    }

    /// Create with default settings.
    pub fn default_bus() -> Self {
        Self::new(1024)
    }

    /// Subscribe to events.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.sender.subscribe()
    }

    /// Add a listener.
    pub async fn add_listener(&self, listener: Arc<dyn EventListener>) {
        self.listeners.write().await.push(listener);
    }

    /// Remove a listener.
    pub async fn remove_listener(&self, index: usize) {
        let mut listeners = self.listeners.write().await;
        if index < listeners.len() {
            listeners.remove(index);
        }
    }

    /// Emit an event.
    pub async fn emit(&self, event: Event) {
        // Add to history
        let mut history = self.history.write().await;
        history.push(event.clone());
        if history.len() > self.history_limit {
            history.remove(0);
        }
        drop(history);

        // Broadcast
        let _ = self.sender.send(event.clone());

        // Notify listeners
        let listeners = self.listeners.read().await;
        for listener in listeners.iter() {
            // Check if listener handles this event
            let handles_type = listener
                .handles()
                .map(|types| types.contains(&event.event_type))
                .unwrap_or(true);

            let handles_category = listener
                .categories()
                .map(|cats| cats.contains(&event.category))
                .unwrap_or(true);

            if handles_type && handles_category {
                listener.on_event(&event).await;
            }
        }
    }

    /// Emit a simple event.
    pub async fn emit_simple(&self, event_type: impl Into<String>) {
        self.emit(Event::new(event_type)).await;
    }

    /// Get event history.
    pub async fn history(&self) -> Vec<Event> {
        self.history.read().await.clone()
    }

    /// Get recent events.
    pub async fn recent(&self, count: usize) -> Vec<Event> {
        let history = self.history.read().await;
        history.iter().rev().take(count).cloned().collect()
    }

    /// Clear history.
    pub async fn clear_history(&self) {
        self.history.write().await.clear();
    }

    /// Get listener count.
    pub async fn listener_count(&self) -> usize {
        self.listeners.read().await.len()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::default_bus()
    }
}

/// Event filter.
#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    /// Event types.
    pub types: Option<Vec<String>>,
    /// Categories.
    pub categories: Option<Vec<EventCategory>>,
    /// Priorities.
    pub priorities: Option<Vec<Priority>>,
    /// Source.
    pub source: Option<String>,
    /// After timestamp.
    pub after: Option<u64>,
    /// Before timestamp.
    pub before: Option<u64>,
}

impl EventFilter {
    /// Create a new filter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Filter by event type.
    pub fn event_type(mut self, event_type: impl Into<String>) -> Self {
        let types = self.types.get_or_insert_with(Vec::new);
        types.push(event_type.into());
        self
    }

    /// Filter by category.
    pub fn category(mut self, category: EventCategory) -> Self {
        let cats = self.categories.get_or_insert_with(Vec::new);
        cats.push(category);
        self
    }

    /// Filter by priority.
    pub fn priority(mut self, priority: Priority) -> Self {
        let prios = self.priorities.get_or_insert_with(Vec::new);
        prios.push(priority);
        self
    }

    /// Filter by source.
    pub fn source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Filter after timestamp.
    pub fn after(mut self, ts: u64) -> Self {
        self.after = Some(ts);
        self
    }

    /// Filter before timestamp.
    pub fn before(mut self, ts: u64) -> Self {
        self.before = Some(ts);
        self
    }

    /// Check if event matches filter.
    pub fn matches(&self, event: &Event) -> bool {
        // Check type
        if let Some(ref types) = self.types
            && !types.contains(&event.event_type)
        {
            return false;
        }

        // Check category
        if let Some(ref cats) = self.categories
            && !cats.contains(&event.category)
        {
            return false;
        }

        // Check priority
        if let Some(ref prios) = self.priorities
            && !prios.contains(&event.priority)
        {
            return false;
        }

        // Check source
        if let Some(ref source) = self.source
            && event.source.as_ref() != Some(source)
        {
            return false;
        }

        // Check timestamp
        if let Some(after) = self.after
            && event.timestamp < after
        {
            return false;
        }

        if let Some(before) = self.before
            && event.timestamp > before
        {
            return false;
        }

        true
    }

    /// Filter events.
    pub fn filter<'a>(&self, events: &'a [Event]) -> Vec<&'a Event> {
        events.iter().filter(|e| self.matches(e)).collect()
    }
}

/// Event aggregator.
pub struct EventAggregator {
    /// Events by type.
    by_type: RwLock<HashMap<String, Vec<Event>>>,
    /// Events by category.
    by_category: RwLock<HashMap<EventCategory, Vec<Event>>>,
    /// Total count.
    total: RwLock<u64>,
}

impl EventAggregator {
    /// Create a new aggregator.
    pub fn new() -> Self {
        Self {
            by_type: RwLock::new(HashMap::new()),
            by_category: RwLock::new(HashMap::new()),
            total: RwLock::new(0),
        }
    }

    /// Add an event.
    pub async fn add(&self, event: Event) {
        // By type
        let mut by_type = self.by_type.write().await;
        by_type
            .entry(event.event_type.clone())
            .or_insert_with(Vec::new)
            .push(event.clone());

        // By category
        let mut by_category = self.by_category.write().await;
        by_category
            .entry(event.category)
            .or_insert_with(Vec::new)
            .push(event);

        // Total
        *self.total.write().await += 1;
    }

    /// Get count by type.
    pub async fn count_by_type(&self, event_type: &str) -> usize {
        self.by_type
            .read()
            .await
            .get(event_type)
            .map(std::vec::Vec::len)
            .unwrap_or(0)
    }

    /// Get count by category.
    pub async fn count_by_category(&self, category: EventCategory) -> usize {
        self.by_category
            .read()
            .await
            .get(&category)
            .map(std::vec::Vec::len)
            .unwrap_or(0)
    }

    /// Get total count.
    pub async fn total(&self) -> u64 {
        *self.total.read().await
    }

    /// Get all types.
    pub async fn types(&self) -> Vec<String> {
        self.by_type.read().await.keys().cloned().collect()
    }

    /// Get statistics.
    pub async fn stats(&self) -> EventStats {
        let by_type = self.by_type.read().await;
        let by_category = self.by_category.read().await;

        EventStats {
            total: *self.total.read().await,
            by_type: by_type.iter().map(|(k, v)| (k.clone(), v.len())).collect(),
            by_category: by_category.iter().map(|(k, v)| (*k, v.len())).collect(),
        }
    }

    /// Clear.
    pub async fn clear(&self) {
        self.by_type.write().await.clear();
        self.by_category.write().await.clear();
        *self.total.write().await = 0;
    }
}

impl Default for EventAggregator {
    fn default() -> Self {
        Self::new()
    }
}

/// Event statistics.
#[derive(Debug, Clone, Serialize)]
pub struct EventStats {
    /// Total events.
    pub total: u64,
    /// Count by type.
    pub by_type: HashMap<String, usize>,
    /// Count by category.
    pub by_category: HashMap<EventCategory, usize>,
}

/// Generate event ID.
fn generate_event_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("evt_{ts:x}")
}

/// Get current timestamp.
fn timestamp_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_creation() {
        let event = Event::new("test_event")
            .category(EventCategory::User)
            .priority(Priority::High)
            .source("test")
            .meta("key", "value");

        assert_eq!(event.event_type, "test_event");
        assert_eq!(event.category, EventCategory::User);
        assert_eq!(event.priority, Priority::High);
        assert_eq!(event.source, Some("test".to_string()));
    }

    #[test]
    fn test_event_payload() {
        let event = Event::new("test").payload(serde_json::json!({"count": 42}));

        let payload: Option<serde_json::Value> = event.get_payload();
        assert!(payload.is_some());
    }

    #[tokio::test]
    async fn test_event_bus() {
        let bus = EventBus::new(16);

        let mut receiver = bus.subscribe();

        bus.emit(Event::new("test_event")).await;

        let event = receiver.recv().await.unwrap();
        assert_eq!(event.event_type, "test_event");
    }

    #[tokio::test]
    async fn test_event_history() {
        let bus = EventBus::new(16);

        bus.emit(Event::new("event1")).await;
        bus.emit(Event::new("event2")).await;

        let history = bus.history().await;
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn test_event_filter() {
        let filter = EventFilter::new()
            .event_type("test")
            .category(EventCategory::User);

        let event = Event::new("test").category(EventCategory::User);
        assert!(filter.matches(&event));

        let event = Event::new("other").category(EventCategory::System);
        assert!(!filter.matches(&event));
    }

    #[tokio::test]
    async fn test_event_aggregator() {
        let aggregator = EventAggregator::new();

        aggregator.add(Event::new("type1")).await;
        aggregator.add(Event::new("type1")).await;
        aggregator.add(Event::new("type2")).await;

        assert_eq!(aggregator.count_by_type("type1").await, 2);
        assert_eq!(aggregator.count_by_type("type2").await, 1);
        assert_eq!(aggregator.total().await, 3);
    }

    #[test]
    fn test_priority_order() {
        assert!(Priority::Low < Priority::Normal);
        assert!(Priority::Normal < Priority::High);
        assert!(Priority::High < Priority::Critical);
    }
}
