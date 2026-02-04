//! Lock command for protecting sessions from deletion.
//!
//! Provides session protection functionality:
//! - Lock sessions to prevent deletion
//! - Unlock sessions
//! - List locked sessions

use anyhow::{Result, bail};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

/// Lock CLI command.
#[derive(Debug, Parser)]
pub struct LockCli {
    #[command(subcommand)]
    pub subcommand: Option<LockSubcommand>,

    /// Session ID to lock (if no subcommand)
    #[arg()]
    pub session_id: Option<String>,
}

/// Lock subcommands.
#[derive(Debug, clap::Subcommand)]
pub enum LockSubcommand {
    /// Lock a session
    #[command(visible_alias = "protect")]
    Add(LockAddArgs),

    /// Unlock a session
    #[command(visible_aliases = ["unprotect", "rm"])]
    Remove(LockRemoveArgs),

    /// List locked sessions
    #[command(visible_alias = "ls")]
    List(LockListArgs),

    /// Check if a session is locked
    Check(LockCheckArgs),
}

/// Arguments for lock add command.
#[derive(Debug, Parser)]
pub struct LockAddArgs {
    /// Session ID(s) to lock
    #[arg(required = true)]
    pub session_ids: Vec<String>,

    /// Reason for locking
    #[arg(long, short = 'r')]
    pub reason: Option<String>,
}

/// Arguments for lock remove command.
#[derive(Debug, Parser)]
pub struct LockRemoveArgs {
    /// Session ID(s) to unlock
    #[arg(required = true)]
    pub session_ids: Vec<String>,

    /// Skip confirmation prompt
    #[arg(long, short = 'y')]
    pub yes: bool,
}

/// Arguments for lock list command.
#[derive(Debug, Parser)]
pub struct LockListArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

/// Arguments for lock check command.
#[derive(Debug, Parser)]
pub struct LockCheckArgs {
    /// Session ID to check
    pub session_id: String,
}

/// Lock entry information.
#[derive(Debug, Serialize, Deserialize)]
struct LockEntry {
    session_id: String,
    locked_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

/// Lock file containing all locked sessions.
#[derive(Debug, Default, Serialize, Deserialize)]
struct LockFile {
    version: u32,
    locked_sessions: Vec<LockEntry>,
}

/// Validate session ID format (must be valid UUID or 8-char prefix)
fn validate_session_id(session_id: &str) -> Result<()> {
    // Check if it's a valid UUID
    if uuid::Uuid::parse_str(session_id).is_ok() {
        return Ok(());
    }

    // Check if it's a valid 8-character hex prefix
    if session_id.len() == 8 && session_id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(());
    }

    bail!(
        "Invalid session ID: '{}'. Expected a full UUID (e.g., '550e8400-e29b-41d4-a716-446655440000') or an 8-character prefix.",
        session_id
    )
}

/// Safely get a string prefix by character count, not byte count.
/// This avoids panics on multi-byte UTF-8 characters.
fn safe_char_prefix(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => &s[..byte_idx],
        None => s, // String has fewer than max_chars characters
    }
}

/// Get the lock file path.
fn get_lock_file_path() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".cortex").join("session_locks.json"))
        .unwrap_or_else(|| PathBuf::from(".cortex/session_locks.json"))
}

/// Load the lock file.
fn load_lock_file() -> Result<LockFile> {
    let path = get_lock_file_path();

    if !path.exists() {
        return Ok(LockFile {
            version: 1,
            locked_sessions: Vec::new(),
        });
    }

    let content = std::fs::read_to_string(&path)?;
    let lock_file: LockFile = serde_json::from_str(&content)?;
    Ok(lock_file)
}

/// Save the lock file.
fn save_lock_file(lock_file: &LockFile) -> Result<()> {
    let path = get_lock_file_path();

    // Create parent directory if it doesn't exist
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let content = serde_json::to_string_pretty(lock_file)?;
    std::fs::write(&path, content)?;
    Ok(())
}

/// Check if a session is locked.
pub fn is_session_locked(session_id: &str) -> bool {
    match load_lock_file() {
        Ok(lock_file) => lock_file.locked_sessions.iter().any(|entry| {
            entry.session_id == session_id
                || session_id.starts_with(safe_char_prefix(&entry.session_id, 8))
        }),
        Err(_) => false,
    }
}

impl LockCli {
    /// Run the lock command.
    pub async fn run(self) -> Result<()> {
        match self.subcommand {
            Some(LockSubcommand::Add(args)) => run_add(args).await,
            Some(LockSubcommand::Remove(args)) => run_remove(args).await,
            Some(LockSubcommand::List(args)) => run_list(args).await,
            Some(LockSubcommand::Check(args)) => run_check(args).await,
            None => {
                // Lock the session directly if ID provided
                if let Some(session_id) = self.session_id {
                    run_add(LockAddArgs {
                        session_ids: vec![session_id],
                        reason: None,
                    })
                    .await
                } else {
                    println!("Session Lock Management");
                    println!("{}", "=".repeat(50));
                    println!();
                    println!("Protect sessions from accidental deletion:");
                    println!();
                    println!("  cortex lock <session-id>          Lock a session");
                    println!("  cortex lock add <session-id>      Lock a session");
                    println!("  cortex lock remove <session-id>   Unlock a session");
                    println!("  cortex lock list                  List locked sessions");
                    println!("  cortex lock check <session-id>    Check if locked");
                    println!();
                    println!("Options:");
                    println!("  --reason <text>  Add reason for locking");
                    println!();
                    println!("Example:");
                    println!("  cortex lock abc12345 --reason \"Important conversation\"");
                    Ok(())
                }
            }
        }
    }
}

async fn run_add(args: LockAddArgs) -> Result<()> {
    // Validate all session IDs first (Issue #3696)
    for session_id in &args.session_ids {
        validate_session_id(session_id)?;
    }

    let mut lock_file = load_lock_file()?;
    let timestamp = chrono::Utc::now().to_rfc3339();

    // Get existing locked session IDs
    let existing_ids: HashSet<String> = lock_file
        .locked_sessions
        .iter()
        .map(|e| e.session_id.clone())
        .collect();

    let mut added = 0;
    let mut skipped = 0;

    for session_id in &args.session_ids {
        if existing_ids.contains(session_id) {
            println!("Session '{}' is already locked.", session_id);
            skipped += 1;
        } else {
            lock_file.locked_sessions.push(LockEntry {
                session_id: session_id.clone(),
                locked_at: timestamp.clone(),
                reason: args.reason.clone(),
            });
            added += 1;
        }
    }

    if added > 0 {
        save_lock_file(&lock_file)?;
        println!(
            "Locked {} session(s){}.",
            added,
            if skipped > 0 {
                format!(" ({} already locked)", skipped)
            } else {
                String::new()
            }
        );
    } else if skipped > 0 {
        println!("All sessions were already locked.");
    }

    Ok(())
}

async fn run_remove(args: LockRemoveArgs) -> Result<()> {
    let mut lock_file = load_lock_file()?;

    // Find sessions to unlock
    let session_ids: HashSet<String> = args.session_ids.iter().cloned().collect();
    let to_remove: Vec<_> = lock_file
        .locked_sessions
        .iter()
        .filter(|e| session_ids.contains(&e.session_id))
        .map(|e| e.session_id.clone())
        .collect();

    if to_remove.is_empty() {
        bail!("None of the specified sessions are locked.");
    }

    if !args.yes {
        println!(
            "Are you sure you want to unlock {} session(s)? (y/N)",
            to_remove.len()
        );
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    lock_file
        .locked_sessions
        .retain(|e| !session_ids.contains(&e.session_id));
    save_lock_file(&lock_file)?;

    println!("Unlocked {} session(s).", to_remove.len());
    Ok(())
}

async fn run_list(args: LockListArgs) -> Result<()> {
    let lock_file = load_lock_file()?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&lock_file.locked_sessions)?
        );
    } else if lock_file.locked_sessions.is_empty() {
        println!("No sessions are locked.");
        println!();
        println!("Use 'cortex lock <session-id>' to protect a session.");
    } else {
        println!("Locked Sessions:");
        println!("{}", "-".repeat(60));

        for entry in &lock_file.locked_sessions {
            let short_id = safe_char_prefix(&entry.session_id, 8);
            println!("  {} - locked at {}", short_id, entry.locked_at);
            if let Some(ref reason) = entry.reason {
                println!("    Reason: {}", reason);
            }
        }

        println!();
        println!(
            "Total: {} locked session(s)",
            lock_file.locked_sessions.len()
        );
    }

    Ok(())
}

async fn run_check(args: LockCheckArgs) -> Result<()> {
    let lock_file = load_lock_file()?;

    let is_locked = lock_file.locked_sessions.iter().any(|e| {
        e.session_id == args.session_id
            || args
                .session_id
                .starts_with(safe_char_prefix(&e.session_id, 8))
    });

    if is_locked {
        println!("Session '{}' is LOCKED.", args.session_id);
        // Find and print the reason if any
        if let Some(entry) = lock_file.locked_sessions.iter().find(|e| {
            e.session_id == args.session_id
                || args
                    .session_id
                    .starts_with(safe_char_prefix(&e.session_id, 8))
        }) && let Some(ref reason) = entry.reason
        {
            println!("Reason: {}", reason);
        }
        std::process::exit(0);
    } else {
        println!("Session '{}' is not locked.", args.session_id);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lock_entry_serialization_with_reason() {
        let entry = LockEntry {
            session_id: "abc123def456".to_string(),
            locked_at: "2024-01-15T10:30:00Z".to_string(),
            reason: Some("Important session".to_string()),
        };

        let json = serde_json::to_string(&entry).expect("Failed to serialize LockEntry");
        let parsed: LockEntry =
            serde_json::from_str(&json).expect("Failed to deserialize LockEntry");

        assert_eq!(parsed.session_id, "abc123def456");
        assert_eq!(parsed.locked_at, "2024-01-15T10:30:00Z");
        assert_eq!(parsed.reason, Some("Important session".to_string()));
    }

    #[test]
    fn test_lock_entry_serialization_without_reason() {
        let entry = LockEntry {
            session_id: "xyz789".to_string(),
            locked_at: "2024-02-20T15:45:00Z".to_string(),
            reason: None,
        };

        let json = serde_json::to_string(&entry).expect("Failed to serialize LockEntry");

        // Verify reason field is omitted when None (due to skip_serializing_if)
        assert!(!json.contains("reason"));

        let parsed: LockEntry =
            serde_json::from_str(&json).expect("Failed to deserialize LockEntry");

        assert_eq!(parsed.session_id, "xyz789");
        assert_eq!(parsed.locked_at, "2024-02-20T15:45:00Z");
        assert_eq!(parsed.reason, None);
    }

    #[test]
    fn test_lock_entry_deserialization_from_json() {
        let json =
            r#"{"session_id":"test123","locked_at":"2024-03-01T00:00:00Z","reason":"Test reason"}"#;
        let entry: LockEntry =
            serde_json::from_str(json).expect("Failed to deserialize LockEntry from JSON");

        assert_eq!(entry.session_id, "test123");
        assert_eq!(entry.locked_at, "2024-03-01T00:00:00Z");
        assert_eq!(entry.reason, Some("Test reason".to_string()));
    }

    #[test]
    fn test_lock_entry_deserialization_without_reason_field() {
        let json = r#"{"session_id":"no_reason_session","locked_at":"2024-04-10T12:00:00Z"}"#;
        let entry: LockEntry =
            serde_json::from_str(json).expect("Failed to deserialize LockEntry without reason");

        assert_eq!(entry.session_id, "no_reason_session");
        assert_eq!(entry.locked_at, "2024-04-10T12:00:00Z");
        assert_eq!(entry.reason, None);
    }

    #[test]
    fn test_lock_file_default() {
        let lock_file = LockFile::default();

        assert_eq!(lock_file.version, 0);
        assert!(lock_file.locked_sessions.is_empty());
    }

    #[test]
    fn test_lock_file_serialization_empty() {
        let lock_file = LockFile {
            version: 1,
            locked_sessions: Vec::new(),
        };

        let json = serde_json::to_string(&lock_file).expect("Failed to serialize empty LockFile");
        let parsed: LockFile =
            serde_json::from_str(&json).expect("Failed to deserialize empty LockFile");

        assert_eq!(parsed.version, 1);
        assert!(parsed.locked_sessions.is_empty());
    }

    #[test]
    fn test_lock_file_serialization_with_entries() {
        let lock_file = LockFile {
            version: 1,
            locked_sessions: vec![
                LockEntry {
                    session_id: "session_one".to_string(),
                    locked_at: "2024-01-01T00:00:00Z".to_string(),
                    reason: Some("First session".to_string()),
                },
                LockEntry {
                    session_id: "session_two".to_string(),
                    locked_at: "2024-01-02T00:00:00Z".to_string(),
                    reason: None,
                },
            ],
        };

        let json =
            serde_json::to_string(&lock_file).expect("Failed to serialize LockFile with entries");
        let parsed: LockFile =
            serde_json::from_str(&json).expect("Failed to deserialize LockFile with entries");

        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.locked_sessions.len(), 2);
        assert_eq!(parsed.locked_sessions[0].session_id, "session_one");
        assert_eq!(
            parsed.locked_sessions[0].reason,
            Some("First session".to_string())
        );
        assert_eq!(parsed.locked_sessions[1].session_id, "session_two");
        assert_eq!(parsed.locked_sessions[1].reason, None);
    }

    #[test]
    fn test_lock_file_deserialization_from_json() {
        let json = r#"{
            "version": 2,
            "locked_sessions": [
                {"session_id": "sess_abc", "locked_at": "2024-05-15T08:00:00Z", "reason": "Production"}
            ]
        }"#;

        let lock_file: LockFile =
            serde_json::from_str(json).expect("Failed to deserialize LockFile from JSON");

        assert_eq!(lock_file.version, 2);
        assert_eq!(lock_file.locked_sessions.len(), 1);
        assert_eq!(lock_file.locked_sessions[0].session_id, "sess_abc");
        assert_eq!(
            lock_file.locked_sessions[0].reason,
            Some("Production".to_string())
        );
    }

    #[test]
    fn test_get_lock_file_path_returns_valid_path() {
        let path = get_lock_file_path();

        // Path should end with session_locks.json
        assert!(path.ends_with("session_locks.json"));

        // Path should include .cortex directory
        let path_str = path.to_string_lossy();
        assert!(path_str.contains(".cortex"));
    }

    #[test]
    fn test_safe_char_prefix_ascii() {
        // ASCII strings should work correctly
        assert_eq!(safe_char_prefix("abcdefghij", 8), "abcdefgh");
        assert_eq!(safe_char_prefix("abc", 8), "abc");
        assert_eq!(safe_char_prefix("", 8), "");
        assert_eq!(safe_char_prefix("12345678", 8), "12345678");
    }

    #[test]
    fn test_safe_char_prefix_utf8_multibyte() {
        // Multi-byte UTF-8 characters should not panic
        // Each emoji is 4 bytes, so 8 chars = 32 bytes
        let emoji_id = "🔥🎉🚀💡🌟✨🎯🔮extra";
        assert_eq!(safe_char_prefix(emoji_id, 8), "🔥🎉🚀💡🌟✨🎯🔮");

        // Mixed ASCII and multi-byte
        let mixed = "ab🔥cd🎉ef";
        assert_eq!(safe_char_prefix(mixed, 4), "ab🔥c");
        assert_eq!(safe_char_prefix(mixed, 8), "ab🔥cd🎉ef");

        // Chinese characters (3 bytes each)
        let chinese = "中文测试会话标识符";
        assert_eq!(safe_char_prefix(chinese, 4), "中文测试");
    }

    #[test]
    fn test_safe_char_prefix_boundary() {
        // Edge cases
        assert_eq!(safe_char_prefix("a", 0), "");
        assert_eq!(safe_char_prefix("a", 1), "a");
        assert_eq!(safe_char_prefix("🔥", 1), "🔥");
        assert_eq!(safe_char_prefix("🔥", 0), "");
    }
}
