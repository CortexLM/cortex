//! Billing command handlers (usage, etc.)

use super::CommandExecutor;
use crate::commands::types::{CommandResult, ParsedCommand};

impl CommandExecutor {
    /// Handles the /usage command with optional date range filters.
    ///
    /// Supports:
    /// - `/usage` - Show usage for current billing period
    /// - `/usage --from YYYY-MM-DD --to YYYY-MM-DD` - Show usage for date range
    pub(super) fn cmd_usage(&self, cmd: &ParsedCommand) -> CommandResult {
        // Parse --from and --to arguments
        let mut from_date: Option<String> = None;
        let mut to_date: Option<String> = None;

        let mut args_iter = cmd.args.iter().peekable();
        while let Some(arg) = args_iter.next() {
            match arg.as_str() {
                "--from" | "-f" => {
                    if let Some(date) = args_iter.next() {
                        // Basic date format validation (YYYY-MM-DD)
                        if Self::is_valid_date_format(date) {
                            from_date = Some(date.clone());
                        } else {
                            return CommandResult::Error(format!(
                                "Invalid date format for --from: '{}'. Use YYYY-MM-DD.",
                                date
                            ));
                        }
                    } else {
                        return CommandResult::Error(
                            "--from requires a date argument (YYYY-MM-DD).".to_string(),
                        );
                    }
                }
                "--to" | "-t" => {
                    if let Some(date) = args_iter.next() {
                        if Self::is_valid_date_format(date) {
                            to_date = Some(date.clone());
                        } else {
                            return CommandResult::Error(format!(
                                "Invalid date format for --to: '{}'. Use YYYY-MM-DD.",
                                date
                            ));
                        }
                    } else {
                        return CommandResult::Error(
                            "--to requires a date argument (YYYY-MM-DD).".to_string(),
                        );
                    }
                }
                _ => {
                    // Ignore unknown arguments
                }
            }
        }

        // Build the async command string with optional date params
        let mut cmd_str = "billing:usage".to_string();
        if let Some(from) = from_date {
            cmd_str.push_str(&format!(":from={}", from));
        }
        if let Some(to) = to_date {
            cmd_str.push_str(&format!(":to={}", to));
        }

        CommandResult::Async(cmd_str)
    }

    /// Validates a date string is in YYYY-MM-DD format.
    pub(super) fn is_valid_date_format(date: &str) -> bool {
        if date.len() != 10 {
            return false;
        }
        let parts: Vec<&str> = date.split('-').collect();
        if parts.len() != 3 {
            return false;
        }
        // Validate year (4 digits), month (2 digits), day (2 digits)
        parts[0].len() == 4
            && parts[0].chars().all(|c| c.is_ascii_digit())
            && parts[1].len() == 2
            && parts[1].chars().all(|c| c.is_ascii_digit())
            && parts[2].len() == 2
            && parts[2].chars().all(|c| c.is_ascii_digit())
    }
}
