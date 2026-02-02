//! State tracking for detecting changes between messages.
//!
//! This module provides functionality to track the prompt context state
//! over time and detect changes that should be notified to the agent.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::context::{PromptContext, TaskStatus};
use crate::state::{AgentState, PromptState};

/// Represents a change detected between states.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StateChange {
    /// An agent was enabled.
    AgentEnabled(String),
    /// An agent was disabled.
    AgentDisabled(String),
    /// An agent's configuration changed.
    AgentConfigChanged(String, String),
    /// A task was added.
    TaskAdded(String),
    /// A task was removed.
    TaskRemoved(String),
    /// A task was completed.
    TaskCompleted(String),
    /// The model was changed.
    ModelChanged(String, String),
    /// The working directory was changed.
    CwdChanged(String, String),
    /// A custom change.
    Custom(String),
}

impl StateChange {
    /// Get a human-readable description of the change.
    pub fn description(&self) -> String {
        match self {
            Self::AgentEnabled(name) => format!("Agent '{}' was enabled", name),
            Self::AgentDisabled(name) => format!("Agent '{}' was disabled", name),
            Self::AgentConfigChanged(name, details) => {
                format!("Agent '{}' configuration changed: {}", name, details)
            }
            Self::TaskAdded(id) => format!("Task '{}' was added", id),
            Self::TaskRemoved(id) => format!("Task '{}' was removed", id),
            Self::TaskCompleted(id) => format!("Task '{}' was completed", id),
            Self::ModelChanged(old, new) => format!("Model changed from '{}' to '{}'", old, new),
            Self::CwdChanged(old, new) => {
                format!("Working directory changed from '{}' to '{}'", old, new)
            }
            Self::Custom(msg) => msg.clone(),
        }
    }
}

/// An event representing a state change with timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeEvent {
    /// The state change.
    pub change: StateChange,
    /// When the change was detected.
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl ChangeEvent {
    /// Create a new change event.
    pub fn new(change: StateChange) -> Self {
        Self {
            change,
            timestamp: chrono::Utc::now(),
        }
    }
}

/// Tracker for monitoring state changes.
///
/// The tracker maintains a snapshot of the previous state and can
/// detect changes when provided with a new context.
#[derive(Debug, Default)]
pub struct StateTracker {
    /// Previous state snapshot.
    previous_state: Option<PromptState>,
    /// History of detected changes.
    change_history: Vec<ChangeEvent>,
    /// Maximum history size.
    max_history: usize,
    /// Whether to track task status changes.
    track_task_status: bool,
    /// Whether to track agent config changes.
    track_agent_config: bool,
}

impl StateTracker {
    /// Create a new state tracker.
    pub fn new() -> Self {
        Self {
            previous_state: None,
            change_history: Vec::new(),
            max_history: 100,
            track_task_status: true,
            track_agent_config: true,
        }
    }

    /// Set the maximum history size.
    pub fn with_max_history(mut self, max: usize) -> Self {
        self.max_history = max;
        self
    }

    /// Set whether to track task status changes.
    pub fn track_task_status(mut self, track: bool) -> Self {
        self.track_task_status = track;
        self
    }

    /// Set whether to track agent config changes.
    pub fn track_agent_config(mut self, track: bool) -> Self {
        self.track_agent_config = track;
        self
    }

    /// Update the tracked state from a context.
    ///
    /// This captures the current state for future comparison.
    pub fn update_state(&mut self, context: &PromptContext) {
        self.previous_state = Some(PromptState::from_context(context));
    }

    /// Detect changes between the previous state and the new context.
    ///
    /// Returns a list of state changes without updating the internal state.
    /// Call `update_state` afterwards if you want to track these changes.
    pub fn detect_changes(&self, new_context: &PromptContext) -> Vec<StateChange> {
        let Some(ref prev) = self.previous_state else {
            // No previous state - no changes to detect
            return Vec::new();
        };

        let new_state = PromptState::from_context(new_context);
        let mut changes = Vec::new();

        // Detect model changes
        if prev.model != new_state.model {
            if let (Some(old), Some(new)) = (&prev.model, &new_state.model) {
                changes.push(StateChange::ModelChanged(old.clone(), new.clone()));
            }
        }

        // Detect cwd changes
        if prev.cwd != new_state.cwd {
            if let (Some(old), Some(new)) = (&prev.cwd, &new_state.cwd) {
                changes.push(StateChange::CwdChanged(old.clone(), new.clone()));
            }
        }

        // Detect main agent changes
        self.detect_agent_changes(&prev.agent, &new_state.agent, &mut changes);

        // Detect subagent changes
        let prev_names: HashSet<_> = prev.subagents.keys().cloned().collect();
        let new_names: HashSet<_> = new_state.subagents.keys().cloned().collect();

        // New subagents
        for name in new_names.difference(&prev_names) {
            if let Some(agent) = new_state.subagents.get(name) {
                if agent.enabled {
                    changes.push(StateChange::AgentEnabled(name.clone()));
                }
            }
        }

        // Removed subagents
        for name in prev_names.difference(&new_names) {
            changes.push(StateChange::AgentDisabled(name.clone()));
        }

        // Changed subagents
        for name in prev_names.intersection(&new_names) {
            if let (Some(prev_agent), Some(new_agent)) =
                (prev.subagents.get(name), new_state.subagents.get(name))
            {
                self.detect_agent_state_changes(prev_agent, new_agent, &mut changes);
            }
        }

        // Detect task changes
        let prev_tasks: HashSet<_> = prev.tasks.keys().cloned().collect();
        let new_tasks: HashSet<_> = new_state.tasks.keys().cloned().collect();

        // New tasks
        for id in new_tasks.difference(&prev_tasks) {
            changes.push(StateChange::TaskAdded(id.clone()));
        }

        // Removed tasks
        for id in prev_tasks.difference(&new_tasks) {
            changes.push(StateChange::TaskRemoved(id.clone()));
        }

        // Task status changes
        if self.track_task_status {
            for id in prev_tasks.intersection(&new_tasks) {
                if let (Some(prev_task), Some(new_task)) =
                    (prev.tasks.get(id), new_state.tasks.get(id))
                {
                    if prev_task.status != new_task.status
                        && new_task.status == TaskStatus::Completed
                    {
                        changes.push(StateChange::TaskCompleted(id.clone()));
                    }
                }
            }
        }

        changes
    }

    /// Detect and record changes, updating the internal state.
    pub fn detect_and_record_changes(&mut self, context: &PromptContext) -> Vec<StateChange> {
        let changes = self.detect_changes(context);

        // Record changes in history
        for change in &changes {
            self.change_history.push(ChangeEvent::new(change.clone()));
        }

        // Trim history
        while self.change_history.len() > self.max_history {
            self.change_history.remove(0);
        }

        // Update state
        self.update_state(context);

        changes
    }

    /// Get the change history.
    pub fn history(&self) -> &[ChangeEvent] {
        &self.change_history
    }

    /// Clear the change history.
    pub fn clear_history(&mut self) {
        self.change_history.clear();
    }

    /// Get the previous state if available.
    pub fn previous_state(&self) -> Option<&PromptState> {
        self.previous_state.as_ref()
    }

    /// Check if we have a previous state to compare against.
    pub fn has_previous_state(&self) -> bool {
        self.previous_state.is_some()
    }

    /// Reset the tracker to initial state.
    pub fn reset(&mut self) {
        self.previous_state = None;
        self.change_history.clear();
    }

    // Helper: Detect changes between two agent states
    fn detect_agent_state_changes(
        &self,
        prev: &AgentState,
        new: &AgentState,
        changes: &mut Vec<StateChange>,
    ) {
        // Enabled/disabled
        if prev.enabled && !new.enabled {
            changes.push(StateChange::AgentDisabled(new.name.clone()));
        } else if !prev.enabled && new.enabled {
            changes.push(StateChange::AgentEnabled(new.name.clone()));
        }

        // Config changes
        if self.track_agent_config && prev.enabled && new.enabled {
            let diffs = new.diff(prev);
            if !diffs.is_empty() && !diffs.iter().all(|d| d.contains("enabled")) {
                changes.push(StateChange::AgentConfigChanged(
                    new.name.clone(),
                    diffs.join(", "),
                ));
            }
        }
    }

    // Helper: Detect changes between main agents
    fn detect_agent_changes(
        &self,
        prev: &Option<AgentState>,
        new: &Option<AgentState>,
        changes: &mut Vec<StateChange>,
    ) {
        match (prev, new) {
            (None, Some(agent)) if agent.enabled => {
                changes.push(StateChange::AgentEnabled(agent.name.clone()));
            }
            (Some(agent), None) if agent.enabled => {
                changes.push(StateChange::AgentDisabled(agent.name.clone()));
            }
            (Some(prev_agent), Some(new_agent)) => {
                self.detect_agent_state_changes(prev_agent, new_agent, changes);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{AgentConfig, TaskConfig};

    #[test]
    fn test_tracker_no_previous_state() {
        let tracker = StateTracker::new();
        let context = PromptContext::new();

        let changes = tracker.detect_changes(&context);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_tracker_model_change() {
        let mut tracker = StateTracker::new();

        let context1 = PromptContext::new().with_model("claude-3");
        tracker.update_state(&context1);

        let context2 = PromptContext::new().with_model("claude-4");
        let changes = tracker.detect_changes(&context2);

        assert!(!changes.is_empty());
        assert!(matches!(&changes[0], StateChange::ModelChanged(_, _)));
    }

    #[test]
    fn test_tracker_agent_enabled() {
        let mut tracker = StateTracker::new();

        let context1 = PromptContext::new();
        tracker.update_state(&context1);

        let context2 = PromptContext::new().with_agent(AgentConfig::new("build"));
        let changes = tracker.detect_changes(&context2);

        assert!(!changes.is_empty());
        assert!(matches!(&changes[0], StateChange::AgentEnabled(_)));
    }

    #[test]
    fn test_tracker_task_added() {
        let mut tracker = StateTracker::new();

        let context1 = PromptContext::new();
        tracker.update_state(&context1);

        let context2 = PromptContext::new().add_task(TaskConfig::new("task-1", "Do something"));
        let changes = tracker.detect_changes(&context2);

        assert!(!changes.is_empty());
        assert!(matches!(&changes[0], StateChange::TaskAdded(_)));
    }

    #[test]
    fn test_tracker_task_completed() {
        let mut tracker = StateTracker::new();

        let context1 =
            PromptContext::new().add_task(TaskConfig::new("task-1", "Do something").in_progress());
        tracker.update_state(&context1);

        let context2 =
            PromptContext::new().add_task(TaskConfig::new("task-1", "Do something").completed());
        let changes = tracker.detect_changes(&context2);

        assert!(!changes.is_empty());
        assert!(matches!(&changes[0], StateChange::TaskCompleted(_)));
    }

    #[test]
    fn test_tracker_record_changes() {
        let mut tracker = StateTracker::new();

        let context1 = PromptContext::new().with_model("model1");
        tracker.update_state(&context1);

        let context2 = PromptContext::new().with_model("model2");
        let changes = tracker.detect_and_record_changes(&context2);

        assert!(!changes.is_empty());
        assert_eq!(tracker.history().len(), 1);
    }

    #[test]
    fn test_tracker_cwd_change() {
        let mut tracker = StateTracker::new();

        let context1 = PromptContext::new().with_cwd("/old/path");
        tracker.update_state(&context1);

        let context2 = PromptContext::new().with_cwd("/new/path");
        let changes = tracker.detect_changes(&context2);

        assert!(!changes.is_empty());
        assert!(matches!(&changes[0], StateChange::CwdChanged(_, _)));
    }

    #[test]
    fn test_change_description() {
        let change = StateChange::AgentEnabled("test".to_string());
        assert!(change.description().contains("test"));
        assert!(change.description().contains("enabled"));
    }
}
