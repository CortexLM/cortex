//! Rollout file reader.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use cortex_protocol::EventMsg;
use serde::Deserialize;

use crate::error::Result;

/// Entry from a rollout file.
#[derive(Debug, Clone, Deserialize)]
pub struct RolloutEntry {
    pub timestamp: String,
    #[serde(flatten)]
    pub item: RolloutItem,
}

/// Rollout item types.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum RolloutItem {
    SessionMeta(SessionMetaEntry),
    EventMsg(EventMsg),
    ResponseItem(serde_json::Value),
    Compacted(serde_json::Value),
    TurnContext(serde_json::Value),
}

/// Session metadata from rollout.
#[derive(Debug, Clone, Deserialize)]
pub struct SessionMetaEntry {
    pub id: String,
    pub parent_id: Option<String>,
    pub fork_point: Option<String>,
    pub timestamp: String,
    pub cwd: String,
    pub model: Option<String>,
    pub cli_version: Option<String>,
    pub instructions: Option<String>,
}

/// Read a rollout file.
///
/// If corrupted entries are found, they are skipped but a warning is logged
/// indicating the number of entries that could not be parsed.
pub fn read_rollout(path: &Path) -> Result<Vec<RolloutEntry>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut entries = Vec::new();
    let mut corrupted_count = 0;
    let mut first_corruption_line = None;

    for (line_num, line_result) in reader.lines().enumerate() {
        let line = line_result?;
        if line.trim().is_empty() {
            continue;
        }

        match serde_json::from_str::<RolloutEntry>(&line) {
            Ok(entry) => entries.push(entry),
            Err(e) => {
                corrupted_count += 1;
                if first_corruption_line.is_none() {
                    first_corruption_line = Some((line_num + 1, e.to_string()));
                }
                tracing::warn!(
                    "Failed to parse rollout entry at line {}: {}",
                    line_num + 1,
                    e
                );
            }
        }
    }

    // Warn if corruption was detected
    if corrupted_count > 0 {
        if let Some((line_num, error_msg)) = first_corruption_line {
            tracing::warn!(
                "Session file may be corrupted: {} entries could not be parsed. \
                 First error at line {}: {}. Some messages may be missing.",
                corrupted_count,
                line_num,
                error_msg
            );
        }
    }

    Ok(entries)
}

/// Get event messages from rollout entries.
pub fn get_events(entries: &[RolloutEntry]) -> Vec<EventMsg> {
    entries
        .iter()
        .filter_map(|e| match &e.item {
            RolloutItem::EventMsg(msg) => Some(msg.clone()),
            _ => None,
        })
        .collect()
}

/// Get session metadata from rollout entries.
pub fn get_session_meta(entries: &[RolloutEntry]) -> Option<&SessionMetaEntry> {
    entries.iter().find_map(|e| match &e.item {
        RolloutItem::SessionMeta(meta) => Some(meta),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_read_rollout() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{"timestamp":"2024-01-01T00:00:00Z","type":"session_meta","payload":{{"id":"test","timestamp":"2024-01-01T00:00:00Z","cwd":"/tmp"}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"timestamp":"2024-01-01T00:00:01Z","type":"event_msg","payload":{{"type":"task_started","model_context_window":128000}}}}"#
        )
        .unwrap();

        let entries = read_rollout(file.path()).unwrap();
        assert_eq!(entries.len(), 2);
    }
}
