//! Feedback command for Cortex CLI.
//!
//! Provides feedback submission functionality:
//! - Submit bug reports
//! - Report good/bad AI results
//! - Submit general feedback

use anyhow::{Result, bail};
use clap::Parser;
use serde::Serialize;
use std::io::{self, Write};
use std::path::PathBuf;

/// Feedback CLI command.
#[derive(Debug, Parser)]
pub struct FeedbackCli {
    #[command(subcommand)]
    pub subcommand: Option<FeedbackSubcommand>,

    /// Feedback message (if no subcommand)
    #[arg(trailing_var_arg = true)]
    pub message: Vec<String>,
}

/// Feedback subcommands.
#[derive(Debug, clap::Subcommand)]
pub enum FeedbackSubcommand {
    /// Report a bug
    Bug(FeedbackBugArgs),

    /// Report a good AI result
    #[command(visible_alias = "thumbs-up")]
    Good(FeedbackResultArgs),

    /// Report a bad AI result
    #[command(visible_alias = "thumbs-down")]
    Bad(FeedbackResultArgs),

    /// Submit general feedback
    #[command(visible_alias = "comment")]
    Submit(FeedbackSubmitArgs),

    /// View feedback history
    History(FeedbackHistoryArgs),
}

/// Arguments for bug report.
#[derive(Debug, Parser)]
pub struct FeedbackBugArgs {
    /// Bug description
    #[arg(trailing_var_arg = true)]
    pub description: Vec<String>,

    /// Include recent logs with the report
    #[arg(long)]
    pub include_logs: bool,

    /// Session ID to attach to the bug report
    #[arg(long, short = 's')]
    pub session: Option<String>,

    /// Output as JSON (for scripting)
    #[arg(long)]
    pub json: bool,
}

/// Arguments for result feedback (good/bad).
#[derive(Debug, Parser)]
pub struct FeedbackResultArgs {
    /// Comment about the result
    #[arg(trailing_var_arg = true)]
    pub comment: Vec<String>,

    /// Session ID to attach
    #[arg(long, short = 's')]
    pub session: Option<String>,
}

/// Arguments for general feedback submission.
#[derive(Debug, Parser)]
pub struct FeedbackSubmitArgs {
    /// Feedback message
    #[arg(trailing_var_arg = true)]
    pub message: Vec<String>,

    /// Include recent logs
    #[arg(long)]
    pub include_logs: bool,

    /// Session ID to attach
    #[arg(long, short = 's')]
    pub session: Option<String>,

    /// Feedback category
    #[arg(long, short = 'c')]
    pub category: Option<String>,
}

/// Arguments for viewing feedback history.
#[derive(Debug, Parser)]
pub struct FeedbackHistoryArgs {
    /// Number of entries to show
    #[arg(long, short = 'n', default_value = "10")]
    pub limit: usize,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

/// Feedback entry for history.
#[derive(Debug, Serialize, serde::Deserialize)]
struct FeedbackEntry {
    id: String,
    timestamp: String,
    category: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
}

/// Get the feedback directory.
fn get_feedback_dir() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".cortex").join("feedback"))
        .unwrap_or_else(|| PathBuf::from(".cortex/feedback"))
}

impl FeedbackCli {
    /// Run the feedback command.
    pub async fn run(self) -> Result<()> {
        match self.subcommand {
            Some(FeedbackSubcommand::Bug(args)) => run_bug(args).await,
            Some(FeedbackSubcommand::Good(args)) => run_good(args).await,
            Some(FeedbackSubcommand::Bad(args)) => run_bad(args).await,
            Some(FeedbackSubcommand::Submit(args)) => run_submit(args).await,
            Some(FeedbackSubcommand::History(args)) => run_history(args).await,
            None => {
                // Handle direct feedback message
                let message = self.message.join(" ");
                if message.is_empty() {
                    println!("Cortex Feedback");
                    println!("{}", "=".repeat(50));
                    println!();
                    println!("Submit feedback to help improve Cortex:");
                    println!();
                    println!("  cortex feedback bug <description>   Report a bug");
                    println!("  cortex feedback good [comment]      Report good result");
                    println!("  cortex feedback bad [comment]       Report bad result");
                    println!("  cortex feedback submit <message>    Submit general feedback");
                    println!();
                    println!("Options:");
                    println!("  --include-logs      Include recent logs");
                    println!("  --session <id>      Attach session to feedback");
                    println!();
                    println!("Example:");
                    println!("  cortex feedback bug \"AI gave wrong answer\" --session abc123");
                } else {
                    // Submit as general feedback
                    run_submit(FeedbackSubmitArgs {
                        message: self.message,
                        include_logs: false,
                        session: None,
                        category: None,
                    })
                    .await?;
                }
                Ok(())
            }
        }
    }
}

async fn run_bug(args: FeedbackBugArgs) -> Result<()> {
    let description = args.description.join(" ");

    if description.is_empty() {
        // Interactive mode
        println!("Bug Report");
        println!("{}", "-".repeat(40));
        println!("Please describe the bug you encountered.");
        println!("Press Enter twice when done.");
        println!();

        let description = read_multiline_input()?;
        if description.is_empty() {
            bail!("Bug description cannot be empty.");
        }

        submit_feedback(
            "bug",
            &description,
            args.session.as_deref(),
            args.include_logs,
        )
        .await?;
    } else {
        submit_feedback(
            "bug",
            &description,
            args.session.as_deref(),
            args.include_logs,
        )
        .await?;
    }

    if args.json {
        println!(
            "{}",
            serde_json::json!({
                "status": "submitted",
                "category": "bug"
            })
        );
    } else {
        println!("Thank you for reporting this bug!");
        println!("Your feedback helps us improve Cortex.");
    }

    Ok(())
}

async fn run_good(args: FeedbackResultArgs) -> Result<()> {
    let comment = args.comment.join(" ");

    submit_feedback("good_result", &comment, args.session.as_deref(), false).await?;

    println!("Thank you for the positive feedback!");
    Ok(())
}

async fn run_bad(args: FeedbackResultArgs) -> Result<()> {
    let comment = args.comment.join(" ");

    if comment.is_empty() {
        println!("What went wrong with the AI result?");
        let comment = read_single_line()?;
        submit_feedback("bad_result", &comment, args.session.as_deref(), false).await?;
    } else {
        submit_feedback("bad_result", &comment, args.session.as_deref(), false).await?;
    }

    println!("Thank you for the feedback! We'll work to improve.");
    Ok(())
}

async fn run_submit(args: FeedbackSubmitArgs) -> Result<()> {
    let message = args.message.join(" ");

    if message.is_empty() {
        // Interactive mode
        println!("Submit Feedback");
        println!("{}", "-".repeat(40));
        println!("Please enter your feedback:");
        println!();

        let message = read_multiline_input()?;
        if message.is_empty() {
            bail!("Feedback message cannot be empty.");
        }

        let category = args.category.as_deref().unwrap_or("general");
        submit_feedback(
            category,
            &message,
            args.session.as_deref(),
            args.include_logs,
        )
        .await?;
    } else {
        let category = args.category.as_deref().unwrap_or("general");
        submit_feedback(
            category,
            &message,
            args.session.as_deref(),
            args.include_logs,
        )
        .await?;
    }

    println!("Thank you for your feedback!");
    Ok(())
}

async fn run_history(args: FeedbackHistoryArgs) -> Result<()> {
    let feedback_dir = get_feedback_dir();

    if !feedback_dir.exists() {
        if args.json {
            println!("[]");
        } else {
            println!("No feedback history found.");
        }
        return Ok(());
    }

    let mut entries = Vec::new();

    // Read feedback files
    if let Ok(dir_entries) = std::fs::read_dir(&feedback_dir) {
        for entry in dir_entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json")
                && let Ok(content) = std::fs::read_to_string(&path)
                && let Ok(entry) = serde_json::from_str::<FeedbackEntry>(&content)
            {
                entries.push(entry);
            }
        }
    }

    // Sort by timestamp (newest first)
    entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    // Apply limit
    entries.truncate(args.limit);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else if entries.is_empty() {
        println!("No feedback history found.");
    } else {
        println!("Feedback History:");
        println!("{}", "-".repeat(60));

        for entry in &entries {
            println!("[{}] {} - {}", entry.timestamp, entry.category, entry.id);
            let truncated = if entry.message.len() > 100 {
                format!("{}...", &entry.message[..100])
            } else {
                entry.message.clone()
            };
            println!("  {}", truncated);
            println!();
        }
    }

    Ok(())
}

/// Submit feedback (save locally and optionally upload).
async fn submit_feedback(
    category: &str,
    message: &str,
    session_id: Option<&str>,
    include_logs: bool,
) -> Result<()> {
    let feedback_dir = get_feedback_dir();
    std::fs::create_dir_all(&feedback_dir)?;

    let id = uuid::Uuid::new_v4().to_string();
    let timestamp = chrono::Utc::now().to_rfc3339();

    let entry = FeedbackEntry {
        id: id.clone(),
        timestamp: timestamp.clone(),
        category: category.to_string(),
        message: message.to_string(),
        session_id: session_id.map(|s| s.to_string()),
    };

    // Save locally
    let filename = format!("feedback-{}.json", &id[..8]);
    let filepath = feedback_dir.join(&filename);
    std::fs::write(&filepath, serde_json::to_string_pretty(&entry)?)?;

    // Log the feedback
    tracing::info!(
        "Feedback submitted: category={}, id={}, include_logs={}",
        category,
        id,
        include_logs
    );

    Ok(())
}

/// Read multiline input from stdin (ends with empty line).
fn read_multiline_input() -> Result<String> {
    let stdin = io::stdin();
    let mut lines = Vec::new();

    loop {
        print!("> ");
        io::stdout().flush()?;

        let mut line = String::new();
        stdin.read_line(&mut line)?;

        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        lines.push(trimmed.to_string());
    }

    Ok(lines.join("\n"))
}

/// Read a single line from stdin.
fn read_single_line() -> Result<String> {
    print!("> ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_feedback_entry_serialization_with_session() {
        let entry = FeedbackEntry {
            id: "test-id-123".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            category: "bug".to_string(),
            message: "Test bug report message".to_string(),
            session_id: Some("session-abc-456".to_string()),
        };

        let json = serde_json::to_string(&entry).expect("serialization should succeed");

        assert!(json.contains("test-id-123"), "JSON should contain the id");
        assert!(
            json.contains("2024-01-01T00:00:00Z"),
            "JSON should contain the timestamp"
        );
        assert!(json.contains("bug"), "JSON should contain the category");
        assert!(
            json.contains("Test bug report message"),
            "JSON should contain the message"
        );
        assert!(
            json.contains("session-abc-456"),
            "JSON should contain the session_id"
        );
        assert!(
            json.contains("session_id"),
            "JSON should contain session_id field name"
        );
    }

    #[test]
    fn test_feedback_entry_serialization_without_session() {
        let entry = FeedbackEntry {
            id: "test-id-789".to_string(),
            timestamp: "2024-06-15T12:30:00Z".to_string(),
            category: "general".to_string(),
            message: "General feedback without session".to_string(),
            session_id: None,
        };

        let json = serde_json::to_string(&entry).expect("serialization should succeed");

        assert!(json.contains("test-id-789"), "JSON should contain the id");
        assert!(json.contains("general"), "JSON should contain the category");
        assert!(
            json.contains("General feedback without session"),
            "JSON should contain the message"
        );
        // session_id should NOT appear when None due to skip_serializing_if
        assert!(
            !json.contains("session_id"),
            "JSON should NOT contain session_id when None"
        );
    }

    #[test]
    fn test_feedback_entry_deserialization_with_session() {
        let json = r#"{
            "id": "deserialize-test-id",
            "timestamp": "2024-03-20T10:15:30Z",
            "category": "bad_result",
            "message": "AI gave incorrect answer",
            "session_id": "session-xyz-789"
        }"#;

        let entry: FeedbackEntry =
            serde_json::from_str(json).expect("deserialization should succeed");

        assert_eq!(entry.id, "deserialize-test-id");
        assert_eq!(entry.timestamp, "2024-03-20T10:15:30Z");
        assert_eq!(entry.category, "bad_result");
        assert_eq!(entry.message, "AI gave incorrect answer");
        assert_eq!(entry.session_id, Some("session-xyz-789".to_string()));
    }

    #[test]
    fn test_feedback_entry_deserialization_without_session() {
        let json = r#"{
            "id": "no-session-id",
            "timestamp": "2024-04-10T08:00:00Z",
            "category": "good_result",
            "message": "AI response was helpful"
        }"#;

        let entry: FeedbackEntry =
            serde_json::from_str(json).expect("deserialization should succeed");

        assert_eq!(entry.id, "no-session-id");
        assert_eq!(entry.timestamp, "2024-04-10T08:00:00Z");
        assert_eq!(entry.category, "good_result");
        assert_eq!(entry.message, "AI response was helpful");
        assert_eq!(entry.session_id, None);
    }

    #[test]
    fn test_feedback_entry_roundtrip_with_session() {
        let original = FeedbackEntry {
            id: "roundtrip-test".to_string(),
            timestamp: "2024-05-25T16:45:00Z".to_string(),
            category: "bug".to_string(),
            message: "Roundtrip test with special chars: é, ñ, 中文".to_string(),
            session_id: Some("session-roundtrip".to_string()),
        };

        let json = serde_json::to_string(&original).expect("serialization should succeed");
        let parsed: FeedbackEntry =
            serde_json::from_str(&json).expect("deserialization should succeed");

        assert_eq!(parsed.id, original.id);
        assert_eq!(parsed.timestamp, original.timestamp);
        assert_eq!(parsed.category, original.category);
        assert_eq!(parsed.message, original.message);
        assert_eq!(parsed.session_id, original.session_id);
    }

    #[test]
    fn test_feedback_entry_roundtrip_without_session() {
        let original = FeedbackEntry {
            id: "roundtrip-no-session".to_string(),
            timestamp: "2024-07-01T00:00:00Z".to_string(),
            category: "general".to_string(),
            message: "Roundtrip test without session".to_string(),
            session_id: None,
        };

        let json = serde_json::to_string(&original).expect("serialization should succeed");
        let parsed: FeedbackEntry =
            serde_json::from_str(&json).expect("deserialization should succeed");

        assert_eq!(parsed.id, original.id);
        assert_eq!(parsed.timestamp, original.timestamp);
        assert_eq!(parsed.category, original.category);
        assert_eq!(parsed.message, original.message);
        assert_eq!(parsed.session_id, original.session_id);
    }

    #[test]
    fn test_feedback_entry_pretty_serialization() {
        let entry = FeedbackEntry {
            id: "pretty-test".to_string(),
            timestamp: "2024-08-12T14:30:00Z".to_string(),
            category: "bug".to_string(),
            message: "Testing pretty print".to_string(),
            session_id: Some("session-pretty".to_string()),
        };

        let pretty_json =
            serde_json::to_string_pretty(&entry).expect("pretty serialization should succeed");

        // Pretty JSON should contain newlines
        assert!(
            pretty_json.contains('\n'),
            "Pretty JSON should contain newlines"
        );

        // Should still be valid and parseable
        let parsed: FeedbackEntry =
            serde_json::from_str(&pretty_json).expect("deserialization should succeed");
        assert_eq!(parsed.id, entry.id);
    }
}
