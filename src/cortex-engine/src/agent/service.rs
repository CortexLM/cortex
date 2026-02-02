//! Agent services and utilities.

use crate::tools::spec::ToolResult;
use serde_json::Value;
use std::collections::VecDeque;

/// Information about a tool call for loop detection.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallInfo {
    /// Tool name.
    pub name: String,
    /// Tool arguments.
    pub arguments: Value,
    /// Result output.
    pub result_output: String,
    /// Result success.
    pub result_success: bool,
}

/// Service for detecting repetitive tool calls (doom loops).
pub struct DoomLoopDetector {
    history: VecDeque<ToolCallInfo>,
    max_history: usize,
    threshold: usize,
}

impl DoomLoopDetector {
    /// Create a new doom loop detector.
    pub fn new(max_history: usize, threshold: usize) -> Self {
        Self {
            history: VecDeque::with_capacity(max_history),
            max_history,
            threshold,
        }
    }

    /// Record a tool call and check if it's a loop.
    /// Returns true if a loop is detected.
    pub fn record_and_check(
        &mut self,
        name: String,
        arguments: Value,
        result: &ToolResult,
    ) -> bool {
        let info = ToolCallInfo {
            name: name.clone(),
            arguments,
            result_output: result.output.clone(),
            result_success: result.success,
        };

        self.history.push_back(info);
        if self.history.len() > self.max_history {
            self.history.pop_front();
        }

        self.is_looping()
    }

    /// Check if the last N calls are identical.
    fn is_looping(&self) -> bool {
        if self.history.len() < self.threshold {
            return false;
        }

        let last_n: Vec<&ToolCallInfo> = self.history.iter().rev().take(self.threshold).collect();
        if last_n.len() < self.threshold {
            return false;
        }

        let first = last_n[0];
        last_n.iter().skip(1).all(|&item| item == first)
    }

    /// Get the last tool name if a loop is detected.
    pub fn last_tool_name(&self) -> Option<String> {
        self.history.back().map(|info| info.name.clone())
    }

    /// Clear the history.
    pub fn clear(&mut self) {
        self.history.clear();
    }
}

impl Default for DoomLoopDetector {
    fn default() -> Self {
        Self::new(10, 3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::spec::ToolResult;
    use serde_json::json;

    #[test]
    fn test_loop_detection() {
        let mut detector = DoomLoopDetector::new(10, 3);
        let name = "test_tool".to_string();
        let args = json!({"arg": 1});
        let result = ToolResult::success("ok");

        assert!(!detector.record_and_check(name.clone(), args.clone(), &result));
        assert!(!detector.record_and_check(name.clone(), args.clone(), &result));
        assert!(detector.record_and_check(name.clone(), args.clone(), &result));
    }

    #[test]
    fn test_not_looping_different_args() {
        let mut detector = DoomLoopDetector::new(10, 3);
        let name = "test_tool".to_string();
        let result = ToolResult::success("ok");

        assert!(!detector.record_and_check(name.clone(), json!({"arg": 1}), &result));
        assert!(!detector.record_and_check(name.clone(), json!({"arg": 2}), &result));
        assert!(!detector.record_and_check(name.clone(), json!({"arg": 1}), &result));
    }

    #[test]
    fn test_not_looping_different_results() {
        let mut detector = DoomLoopDetector::new(10, 3);
        let name = "test_tool".to_string();
        let args = json!({"arg": 1});

        assert!(!detector.record_and_check(
            name.clone(),
            args.clone(),
            &ToolResult::success("ok1")
        ));
        assert!(!detector.record_and_check(
            name.clone(),
            args.clone(),
            &ToolResult::success("ok2")
        ));
        assert!(!detector.record_and_check(
            name.clone(),
            args.clone(),
            &ToolResult::success("ok1")
        ));
    }
}
