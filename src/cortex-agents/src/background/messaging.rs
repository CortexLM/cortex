//! Inter-agent messaging system.
//!
//! Provides asynchronous communication between background agents
//! using channels and a message broker.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, RwLock};

/// Content types for inter-agent messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageContent {
    /// Simple notification message.
    Notify(String),

    /// Request expecting a response.
    Request {
        /// Unique request ID for correlation.
        id: String,
        /// Request payload.
        payload: String,
    },

    /// Response to a request.
    Response {
        /// ID of the original request.
        request_id: String,
        /// Response payload.
        payload: String,
    },

    /// Shared data between agents.
    Data {
        /// Data key.
        key: String,
        /// Data value.
        value: serde_json::Value,
    },

    /// Agent status update.
    Status {
        /// Status code.
        code: String,
        /// Status message.
        message: String,
    },

    /// Task delegation request.
    Delegate {
        /// Task description.
        task: String,
        /// Optional context.
        context: Option<String>,
    },

    /// Task completion notification.
    TaskComplete {
        /// Task ID.
        task_id: String,
        /// Result summary.
        result: String,
        /// Success flag.
        success: bool,
    },
}

impl MessageContent {
    /// Creates a notify message.
    pub fn notify(message: impl Into<String>) -> Self {
        MessageContent::Notify(message.into())
    }

    /// Creates a request message.
    pub fn request(id: impl Into<String>, payload: impl Into<String>) -> Self {
        MessageContent::Request {
            id: id.into(),
            payload: payload.into(),
        }
    }

    /// Creates a response message.
    pub fn response(request_id: impl Into<String>, payload: impl Into<String>) -> Self {
        MessageContent::Response {
            request_id: request_id.into(),
            payload: payload.into(),
        }
    }

    /// Creates a data message.
    pub fn data(key: impl Into<String>, value: serde_json::Value) -> Self {
        MessageContent::Data {
            key: key.into(),
            value,
        }
    }

    /// Creates a status message.
    pub fn status(code: impl Into<String>, message: impl Into<String>) -> Self {
        MessageContent::Status {
            code: code.into(),
            message: message.into(),
        }
    }

    /// Creates a delegate message.
    pub fn delegate(task: impl Into<String>, context: Option<String>) -> Self {
        MessageContent::Delegate {
            task: task.into(),
            context,
        }
    }

    /// Creates a task complete message.
    pub fn task_complete(
        task_id: impl Into<String>,
        result: impl Into<String>,
        success: bool,
    ) -> Self {
        MessageContent::TaskComplete {
            task_id: task_id.into(),
            result: result.into(),
            success,
        }
    }

    /// Returns true if this is a request expecting a response.
    pub fn expects_response(&self) -> bool {
        matches!(self, MessageContent::Request { .. })
    }

    /// Returns the request ID if this is a request.
    pub fn request_id(&self) -> Option<&str> {
        match self {
            MessageContent::Request { id, .. } => Some(id),
            MessageContent::Response { request_id, .. } => Some(request_id),
            _ => None,
        }
    }
}

/// A message sent between agents.
#[derive(Debug, Clone)]
pub struct AgentMessage {
    /// Sender agent ID.
    pub from: String,
    /// Recipient agent ID (or "*" for broadcast).
    pub to: String,
    /// Message content.
    pub content: MessageContent,
    /// When the message was sent.
    pub timestamp: Instant,
    /// Message priority (higher = more important).
    pub priority: i32,
}

impl AgentMessage {
    /// Creates a new message.
    pub fn new(from: impl Into<String>, to: impl Into<String>, content: MessageContent) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            content,
            timestamp: Instant::now(),
            priority: 0,
        }
    }

    /// Creates a broadcast message to all agents.
    pub fn broadcast(from: impl Into<String>, content: MessageContent) -> Self {
        Self::new(from, "*", content)
    }

    /// Sets the priority.
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Returns true if this is a broadcast message.
    pub fn is_broadcast(&self) -> bool {
        self.to == "*"
    }

    /// Returns the age of this message.
    pub fn age(&self) -> Duration {
        self.timestamp.elapsed()
    }
}

/// Mailbox for an agent to send and receive messages.
pub struct AgentMailbox {
    /// This agent's ID.
    agent_id: String,
    /// Inbox for receiving messages.
    inbox: mpsc::Receiver<AgentMessage>,
    /// Outbox for sending messages (to the broker).
    outbox: mpsc::Sender<AgentMessage>,
}

impl AgentMailbox {
    /// Creates a new mailbox.
    fn new(
        agent_id: String,
        inbox: mpsc::Receiver<AgentMessage>,
        outbox: mpsc::Sender<AgentMessage>,
    ) -> Self {
        Self {
            agent_id,
            inbox,
            outbox,
        }
    }

    /// Returns this agent's ID.
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    /// Sends a message to another agent.
    pub async fn send(&self, to: &str, content: MessageContent) -> Result<(), MailboxError> {
        let msg = AgentMessage::new(&self.agent_id, to, content);
        self.outbox
            .send(msg)
            .await
            .map_err(|_| MailboxError::SendFailed)
    }

    /// Sends a message with priority.
    pub async fn send_priority(
        &self,
        to: &str,
        content: MessageContent,
        priority: i32,
    ) -> Result<(), MailboxError> {
        let msg = AgentMessage::new(&self.agent_id, to, content).with_priority(priority);
        self.outbox
            .send(msg)
            .await
            .map_err(|_| MailboxError::SendFailed)
    }

    /// Broadcasts a message to all agents.
    pub async fn broadcast(&self, content: MessageContent) -> Result<(), MailboxError> {
        let msg = AgentMessage::broadcast(&self.agent_id, content);
        self.outbox
            .send(msg)
            .await
            .map_err(|_| MailboxError::SendFailed)
    }

    /// Tries to receive a message without blocking.
    pub fn try_recv(&mut self) -> Option<AgentMessage> {
        self.inbox.try_recv().ok()
    }

    /// Receives a message with timeout.
    pub async fn recv_timeout(&mut self, timeout: Duration) -> Option<AgentMessage> {
        tokio::time::timeout(timeout, self.inbox.recv())
            .await
            .ok()
            .flatten()
    }

    /// Receives the next message (blocks until available).
    pub async fn recv(&mut self) -> Option<AgentMessage> {
        self.inbox.recv().await
    }

    /// Drains all pending messages.
    pub fn drain(&mut self) -> Vec<AgentMessage> {
        let mut messages = Vec::new();
        while let Some(msg) = self.try_recv() {
            messages.push(msg);
        }
        messages
    }

    /// Returns true if there are pending messages.
    pub fn has_messages(&mut self) -> bool {
        // Try to peek without consuming
        self.inbox.try_recv().is_ok()
    }
}

/// Errors from mailbox operations.
#[derive(Debug, thiserror::Error)]
pub enum MailboxError {
    /// Failed to send message.
    #[error("Failed to send message")]
    SendFailed,

    /// Agent not found.
    #[error("Agent not found: {0}")]
    AgentNotFound(String),

    /// Mailbox closed.
    #[error("Mailbox closed")]
    Closed,
}

/// Message broker for routing messages between agents.
pub struct AgentMessageBroker {
    /// Registered agent inboxes.
    inboxes: Arc<RwLock<HashMap<String, mpsc::Sender<AgentMessage>>>>,
    /// Central outbox for all messages.
    central_rx: Arc<RwLock<Option<mpsc::Receiver<AgentMessage>>>>,
    /// Central sender for creating mailboxes.
    central_tx: mpsc::Sender<AgentMessage>,
    /// Message buffer capacity.
    buffer_capacity: usize,
}

impl AgentMessageBroker {
    /// Creates a new message broker.
    pub fn new(buffer_capacity: usize) -> Self {
        let (central_tx, central_rx) = mpsc::channel(buffer_capacity * 10);

        Self {
            inboxes: Arc::new(RwLock::new(HashMap::new())),
            central_rx: Arc::new(RwLock::new(Some(central_rx))),
            central_tx,
            buffer_capacity,
        }
    }

    /// Creates a broker with default settings.
    pub fn default_broker() -> Self {
        Self::new(100)
    }

    /// Registers an agent and creates its mailbox.
    pub async fn register(&self, agent_id: impl Into<String>) -> AgentMailbox {
        let agent_id = agent_id.into();
        let (inbox_tx, inbox_rx) = mpsc::channel(self.buffer_capacity);

        // Store the inbox sender for routing
        let mut inboxes = self.inboxes.write().await;
        inboxes.insert(agent_id.clone(), inbox_tx);

        AgentMailbox::new(agent_id, inbox_rx, self.central_tx.clone())
    }

    /// Unregisters an agent.
    pub async fn unregister(&self, agent_id: &str) {
        let mut inboxes = self.inboxes.write().await;
        inboxes.remove(agent_id);
    }

    /// Returns the number of registered agents.
    pub async fn agent_count(&self) -> usize {
        let inboxes = self.inboxes.read().await;
        inboxes.len()
    }

    /// Lists all registered agent IDs.
    pub async fn list_agents(&self) -> Vec<String> {
        let inboxes = self.inboxes.read().await;
        inboxes.keys().cloned().collect()
    }

    /// Runs the message broker routing loop.
    ///
    /// This should be spawned as a background task.
    pub async fn run(&self) {
        // Take ownership of the receiver
        let mut central_rx = {
            let mut rx_lock = self.central_rx.write().await;
            rx_lock.take()
        };

        let Some(ref mut rx) = central_rx else {
            tracing::warn!("Message broker receiver already taken");
            return;
        };

        while let Some(msg) = rx.recv().await {
            self.route_message(msg).await;
        }

        tracing::info!("Message broker stopped");
    }

    /// Routes a message to its recipient(s).
    async fn route_message(&self, msg: AgentMessage) {
        let inboxes = self.inboxes.read().await;

        if msg.is_broadcast() {
            // Send to all agents except sender
            for (id, inbox) in inboxes.iter() {
                if id != &msg.from {
                    let _ = inbox.send(msg.clone()).await;
                }
            }
        } else {
            // Send to specific agent
            if let Some(inbox) = inboxes.get(&msg.to) {
                let _ = inbox.send(msg).await;
            } else {
                tracing::warn!("Message to unknown agent: {}", msg.to);
            }
        }
    }

    /// Sends a message directly (bypassing mailbox).
    pub async fn send_direct(
        &self,
        from: &str,
        to: &str,
        content: MessageContent,
    ) -> Result<(), MailboxError> {
        let msg = AgentMessage::new(from, to, content);
        let inboxes = self.inboxes.read().await;

        if let Some(inbox) = inboxes.get(to) {
            inbox.send(msg).await.map_err(|_| MailboxError::SendFailed)
        } else {
            Err(MailboxError::AgentNotFound(to.to_string()))
        }
    }

    /// Broadcasts a message to all agents.
    pub async fn broadcast(&self, from: &str, content: MessageContent) {
        let msg = AgentMessage::broadcast(from, content);
        let inboxes = self.inboxes.read().await;

        for (id, inbox) in inboxes.iter() {
            if id != from {
                let _ = inbox.send(msg.clone()).await;
            }
        }
    }
}

impl Default for AgentMessageBroker {
    fn default() -> Self {
        Self::new(100)
    }
}

/// Creates a pair of connected mailboxes for testing.
#[cfg(test)]
pub fn create_connected_mailboxes() -> (AgentMailbox, AgentMailbox) {
    let (tx1, rx1) = mpsc::channel(100);
    let (tx2, rx2) = mpsc::channel(100);

    // Cross-connect: agent1's outbox -> agent2's inbox and vice versa
    let mailbox1 = AgentMailbox::new("agent1".to_string(), rx2, tx1);
    let mailbox2 = AgentMailbox::new("agent2".to_string(), rx1, tx2);

    (mailbox1, mailbox2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_direct_message() {
        let broker = AgentMessageBroker::new(100);

        let mut mailbox1 = broker.register("agent1").await;
        let _mailbox2 = broker.register("agent2").await;

        // Send direct message
        broker
            .send_direct("agent2", "agent1", MessageContent::notify("Hello"))
            .await
            .unwrap();

        // Receive
        let msg = mailbox1.recv_timeout(Duration::from_secs(1)).await;
        assert!(msg.is_some());
        let msg = msg.unwrap();
        assert_eq!(msg.from, "agent2");
        assert!(matches!(msg.content, MessageContent::Notify(_)));
    }

    #[tokio::test]
    async fn test_broadcast() {
        let broker = AgentMessageBroker::new(100);

        let mut mailbox1 = broker.register("agent1").await;
        let mut mailbox2 = broker.register("agent2").await;
        let _mailbox3 = broker.register("agent3").await;

        // Broadcast from agent3
        broker
            .broadcast("agent3", MessageContent::notify("Broadcast"))
            .await;

        // Both agent1 and agent2 should receive
        let msg1 = mailbox1.recv_timeout(Duration::from_millis(100)).await;
        let msg2 = mailbox2.recv_timeout(Duration::from_millis(100)).await;

        assert!(msg1.is_some());
        assert!(msg2.is_some());
    }

    #[tokio::test]
    async fn test_mailbox_send_receive() {
        let (mailbox1, mut mailbox2) = create_connected_mailboxes();

        // Send from 1 to 2
        mailbox1
            .send("agent2", MessageContent::notify("Hello from 1"))
            .await
            .unwrap();

        // Wait a bit and try to receive (note: cross-connected for testing)
        tokio::time::sleep(Duration::from_millis(10)).await;

        // In the cross-connected setup, messages go the other way
        let msg = mailbox2.recv_timeout(Duration::from_millis(100)).await;
        assert!(msg.is_some());
    }

    #[tokio::test]
    async fn test_message_content_types() {
        let notify = MessageContent::notify("test");
        assert!(!notify.expects_response());

        let request = MessageContent::request("req-1", "payload");
        assert!(request.expects_response());
        assert_eq!(request.request_id(), Some("req-1"));

        let response = MessageContent::response("req-1", "response");
        assert!(!response.expects_response());
        assert_eq!(response.request_id(), Some("req-1"));

        let data = MessageContent::data("key", serde_json::json!({"foo": "bar"}));
        assert!(!data.expects_response());
    }

    #[tokio::test]
    async fn test_agent_registration() {
        let broker = AgentMessageBroker::new(100);

        broker.register("agent1").await;
        broker.register("agent2").await;
        broker.register("agent3").await;

        assert_eq!(broker.agent_count().await, 3);

        let agents = broker.list_agents().await;
        assert!(agents.contains(&"agent1".to_string()));
        assert!(agents.contains(&"agent2".to_string()));
        assert!(agents.contains(&"agent3".to_string()));

        broker.unregister("agent2").await;
        assert_eq!(broker.agent_count().await, 2);
    }

    #[tokio::test]
    async fn test_message_priority() {
        let msg = AgentMessage::new("sender", "receiver", MessageContent::notify("test"))
            .with_priority(10);

        assert_eq!(msg.priority, 10);
        assert!(!msg.is_broadcast());
    }

    #[tokio::test]
    async fn test_broadcast_message() {
        let msg = AgentMessage::broadcast("sender", MessageContent::notify("broadcast"));

        assert!(msg.is_broadcast());
        assert_eq!(msg.to, "*");
    }

    #[test]
    fn test_message_age() {
        let msg = AgentMessage::new("sender", "receiver", MessageContent::notify("test"));
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert!(msg.age() >= Duration::from_millis(10));
    }

    #[tokio::test]
    async fn test_drain_messages() {
        let (_mailbox1, _mailbox2) = create_connected_mailboxes();

        // Send multiple messages (these go to mailbox2's rx which is mailbox1's tx in test setup)
        // The cross-connected test setup means we need to receive from the right side
        let broker = AgentMessageBroker::new(100);
        let mut mailbox = broker.register("test").await;

        broker
            .send_direct("other", "test", MessageContent::notify("msg1"))
            .await
            .unwrap();
        broker
            .send_direct("other", "test", MessageContent::notify("msg2"))
            .await
            .unwrap();
        broker
            .send_direct("other", "test", MessageContent::notify("msg3"))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;

        let messages = mailbox.drain();
        assert_eq!(messages.len(), 3);
    }
}
