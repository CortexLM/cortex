//! Doom loop detection and prevention.
//!
//! Detects when the agent is stuck in infinite loops of tool calls
//! and prompts the user for intervention.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// Configuration for doom loop detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoomLoopConfig {
    /// Maximum number of identical tool calls before triggering.
    pub max_identical_calls: usize,
    /// Time window for counting calls (in seconds).
    pub window_seconds: u64,
    /// Maximum total tool calls in a turn before warning.
    pub max_calls_per_turn: usize,
    /// Whether doom loop protection is enabled.
    pub enabled: bool,
    /// Default action: "ask", "allow", or "deny".
    pub default_action: String,
}

impl Default for DoomLoopConfig {
    fn default() -> Self {
        Self {
            max_identical_calls: 5,
            window_seconds: 60,
            max_calls_per_turn: 50,
            enabled: true,
            default_action: "ask".to_string(),
        }
    }
}

/// A recorded tool call for loop detection.
#[derive(Debug, Clone)]
struct ToolCallRecord {
    /// Hash combining tool name and parameters for deduplication.
    hash: u64,
    /// Timestamp when the call was recorded.
    timestamp: Instant,
}

/// Detects doom loops (infinite tool call patterns) using hash-based detection.
///
/// This detector uses parameter hashes to identify repeated tool calls within
/// a time window. For simpler exact-match detection, see `crate::agent::DoomLoopDetector`.
#[derive(Debug)]
pub struct HashDoomLoopDetector {
    config: DoomLoopConfig,
    /// Tool calls in current session with timestamps for expiration.
    call_history: Vec<ToolCallRecord>,
    /// Count of identical calls by hash. Cleaned up alongside call_history.
    identical_call_counts: HashMap<u64, usize>,
    /// Total calls in current turn.
    turn_call_count: usize,
    /// Patterns that have been allowed to continue (O(1) lookup).
    allowed_patterns: HashSet<u64>,
}

impl HashDoomLoopDetector {
    /// Create a new detector with config.
    pub fn with_config(config: DoomLoopConfig) -> Self {
        Self {
            config,
            call_history: Vec::new(),
            identical_call_counts: HashMap::new(),
            turn_call_count: 0,
            allowed_patterns: HashSet::new(),
        }
    }

    /// Create a new detector with simple parameters (for compatibility).
    ///
    /// # Arguments
    /// * `max_history` - Maximum call history size (maps to max_calls_per_turn)
    /// * `threshold` - Number of identical calls before triggering (maps to max_identical_calls)
    pub fn new(max_history: usize, threshold: usize) -> Self {
        Self::with_config(DoomLoopConfig {
            max_identical_calls: threshold,
            max_calls_per_turn: max_history,
            ..Default::default()
        })
    }

    /// Create with default config.
    pub fn default_config() -> Self {
        Self::with_config(DoomLoopConfig::default())
    }

    /// Record a tool call and check for doom loop.
    pub fn record_call(&mut self, tool_name: &str, params: &serde_json::Value) -> DoomLoopCheck {
        if !self.config.enabled {
            return DoomLoopCheck::Ok;
        }

        self.turn_call_count += 1;

        // Check total calls per turn
        if self.turn_call_count > self.config.max_calls_per_turn {
            return DoomLoopCheck::TooManyCalls {
                count: self.turn_call_count,
                max: self.config.max_calls_per_turn,
            };
        }

        // Calculate hash for this call
        let params_str = serde_json::to_string(params).unwrap_or_default();
        let hash = Self::hash_call(tool_name, &params_str);

        // Check if this pattern was already allowed
        if self.allowed_patterns.contains(&hash) {
            return DoomLoopCheck::Ok;
        }

        // Clean old entries outside the window and their associated counts
        let window = Duration::from_secs(self.config.window_seconds);
        let now = Instant::now();

        // Collect hashes of expired records to clean up counts
        let expired_hashes: Vec<u64> = self
            .call_history
            .iter()
            .filter(|r| now.duration_since(r.timestamp) >= window)
            .map(|r| r.hash)
            .collect();

        // Decrement counts for expired records
        for expired_hash in expired_hashes {
            if let Some(count) = self.identical_call_counts.get_mut(&expired_hash) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.identical_call_counts.remove(&expired_hash);
                }
            }
        }

        // Remove expired records
        self.call_history
            .retain(|r| now.duration_since(r.timestamp) < window);

        // Record this call
        self.call_history.push(ToolCallRecord {
            hash,
            timestamp: now,
        });

        // Count identical calls
        let count = self.identical_call_counts.entry(hash).or_insert(0);
        *count += 1;

        if *count >= self.config.max_identical_calls {
            return DoomLoopCheck::LoopDetected {
                tool: tool_name.to_string(),
                count: *count,
                hash,
            };
        }

        DoomLoopCheck::Ok
    }

    /// Allow a pattern to continue (user said "always").
    pub fn allow_pattern(&mut self, hash: u64) {
        self.allowed_patterns.insert(hash);
    }

    /// Reset for a new turn.
    pub fn reset_turn(&mut self) {
        self.turn_call_count = 0;
    }

    /// Reset completely for a new session.
    pub fn reset(&mut self) {
        self.call_history.clear();
        self.identical_call_counts.clear();
        self.turn_call_count = 0;
        self.allowed_patterns.clear();
    }

    /// Hash a tool call for comparison.
    fn hash_call(tool_name: &str, params_str: &str) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        tool_name.hash(&mut hasher);
        params_str.hash(&mut hasher);
        hasher.finish()
    }

    /// Get current turn call count.
    pub fn turn_call_count(&self) -> usize {
        self.turn_call_count
    }
}

/// Result of a doom loop check.
#[derive(Debug, Clone)]
pub enum DoomLoopCheck {
    /// No loop detected.
    Ok,
    /// Loop detected - same call repeated too many times.
    LoopDetected {
        tool: String,
        count: usize,
        hash: u64,
    },
    /// Too many total calls in this turn.
    TooManyCalls { count: usize, max: usize },
}

impl DoomLoopCheck {
    /// Check if this is a warning.
    pub fn is_warning(&self) -> bool {
        !matches!(self, DoomLoopCheck::Ok)
    }

    /// Get a message for the user.
    pub fn message(&self) -> Option<String> {
        match self {
            DoomLoopCheck::Ok => None,
            DoomLoopCheck::LoopDetected { tool, count, .. } => Some(format!(
                "Potential doom loop detected: '{}' called {} times with same parameters. \
                     The agent may be stuck. Continue?",
                tool, count
            )),
            DoomLoopCheck::TooManyCalls { count, max } => Some(format!(
                "Too many tool calls in this turn ({}/{}). \
                     The agent may be stuck in a loop. Continue?",
                count, max
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_doom_loop_detection() {
        let config = DoomLoopConfig {
            max_identical_calls: 3,
            window_seconds: 60,
            max_calls_per_turn: 10,
            enabled: true,
            default_action: "ask".to_string(),
        };

        let mut detector = HashDoomLoopDetector::with_config(config);

        // Same call multiple times
        let params = json!({"file": "test.txt"});

        assert!(matches!(
            detector.record_call("read", &params),
            DoomLoopCheck::Ok
        ));
        assert!(matches!(
            detector.record_call("read", &params),
            DoomLoopCheck::Ok
        ));

        // Third identical call should trigger
        match detector.record_call("read", &params) {
            DoomLoopCheck::LoopDetected { tool, count, .. } => {
                assert_eq!(tool, "read");
                assert_eq!(count, 3);
            }
            _ => panic!("Expected loop detection"),
        }
    }

    #[test]
    fn test_different_params_no_loop() {
        let mut detector = HashDoomLoopDetector::default_config();

        for i in 0..10 {
            let params = json!({"file": format!("test{}.txt", i)});
            assert!(matches!(
                detector.record_call("read", &params),
                DoomLoopCheck::Ok
            ));
        }
    }

    #[test]
    fn test_allow_pattern() {
        let config = DoomLoopConfig {
            max_identical_calls: 2,
            ..Default::default()
        };

        let mut detector = HashDoomLoopDetector::with_config(config);
        let params = json!({"file": "test.txt"});

        detector.record_call("read", &params);

        if let DoomLoopCheck::LoopDetected { hash, .. } = detector.record_call("read", &params) {
            detector.allow_pattern(hash);
            // Now it should be ok
            assert!(matches!(
                detector.record_call("read", &params),
                DoomLoopCheck::Ok
            ));
        }
    }
}
