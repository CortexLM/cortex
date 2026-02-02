//! Command parser for cortex-tui slash commands.
//!
//! This module handles parsing of slash command input strings into
//! structured `ParsedCommand` objects. It supports:
//!
//! - Simple commands: `/quit`
//! - Commands with arguments: `/help topic`
//! - Quoted arguments: `/search "hello world"`
//! - Mixed arguments: `/add file.txt "path with spaces"`

use super::types::ParsedCommand;

// ============================================================
// COMMAND PARSER
// ============================================================

/// Parser for slash commands.
///
/// Handles parsing of command strings like `/cmd arg1 "arg 2"` into
/// structured `ParsedCommand` objects.
pub struct CommandParser;

impl CommandParser {
    /// Parse an input string into a `ParsedCommand`.
    ///
    /// Returns `None` if the input is not a valid command (doesn't start with `/`).
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let cmd = CommandParser::parse("/help topic").unwrap();
    /// assert_eq!(cmd.name, "help");
    /// assert_eq!(cmd.args, vec!["topic"]);
    /// ```
    pub fn parse(input: &str) -> Option<ParsedCommand> {
        let input = input.trim();

        // Must start with /
        if !input.starts_with('/') {
            return None;
        }

        // Handle empty command
        let rest = &input[1..];
        if rest.is_empty() {
            return None;
        }

        // Split into command name and arguments
        let (name, args_str) = match rest.find(char::is_whitespace) {
            Some(pos) => {
                let name = &rest[..pos];
                let args = rest[pos..].trim();
                (name, args)
            }
            None => (rest, ""),
        };

        // Command name must be non-empty and alphanumeric (plus hyphen/underscore)
        if name.is_empty() || !Self::is_valid_command_name(name) {
            return None;
        }

        // Parse arguments
        let args = if args_str.is_empty() {
            Vec::new()
        } else {
            Self::split_args(args_str)
        };

        Some(ParsedCommand::new(
            name.to_lowercase(),
            args,
            input.to_string(),
        ))
    }

    /// Check if the input starts with a slash command.
    ///
    /// Returns `true` if the input appears to be a command (starts with `/`
    /// followed by alphanumeric characters).
    pub fn is_command(input: &str) -> bool {
        let input = input.trim();
        if !input.starts_with('/') {
            return false;
        }

        // Check that there's something after the /
        let rest = &input[1..];
        if rest.is_empty() {
            return false;
        }

        // First character after / should be alphanumeric or valid command char
        rest.chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '?' || c == '_')
    }

    /// Check if a command name is valid.
    ///
    /// Valid names contain only alphanumeric characters, hyphens, and underscores.
    /// Special case: `?` is allowed as a single-character command.
    fn is_valid_command_name(name: &str) -> bool {
        if name == "?" {
            return true;
        }
        name.chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    }

    /// Split argument string respecting quotes (shell-like parsing).
    ///
    /// Handles:
    /// - Unquoted arguments separated by whitespace
    /// - Double-quoted arguments containing spaces
    /// - Single-quoted arguments containing spaces
    /// - Escaped quotes within quoted strings
    ///
    /// This method is public to allow reuse in command handlers that need
    /// to parse argument strings with the same quote-aware logic.
    pub fn split_args(args_str: &str) -> Vec<String> {
        let mut args = Vec::new();
        let mut current = String::new();
        let mut chars = args_str.chars().peekable();
        let mut in_double_quote = false;
        let mut in_single_quote = false;

        while let Some(c) = chars.next() {
            match c {
                '"' if !in_single_quote => {
                    if in_double_quote {
                        // End of double-quoted string
                        in_double_quote = false;
                    } else {
                        // Start of double-quoted string
                        in_double_quote = true;
                    }
                }
                '\'' if !in_double_quote => {
                    if in_single_quote {
                        // End of single-quoted string
                        in_single_quote = false;
                    } else {
                        // Start of single-quoted string
                        in_single_quote = true;
                    }
                }
                '\\' if (in_double_quote || in_single_quote) => {
                    // Handle escape sequences in quoted strings
                    if let Some(&next) = chars.peek() {
                        match next {
                            '"' | '\'' | '\\' => {
                                current.push(chars.next().unwrap());
                            }
                            'n' => {
                                chars.next();
                                current.push('\n');
                            }
                            't' => {
                                chars.next();
                                current.push('\t');
                            }
                            _ => {
                                current.push('\\');
                            }
                        }
                    } else {
                        current.push('\\');
                    }
                }
                c if c.is_whitespace() && !in_double_quote && !in_single_quote => {
                    // End of argument (if not empty)
                    if !current.is_empty() {
                        args.push(current);
                        current = String::new();
                    }
                }
                _ => {
                    current.push(c);
                }
            }
        }

        // Don't forget the last argument
        if !current.is_empty() {
            args.push(current);
        }

        args
    }

    /// Extract the command name from input for completion purposes.
    ///
    /// Returns the partial command name (without the leading /) if the input
    /// appears to be starting a command.
    pub fn extract_partial_command(input: &str) -> Option<&str> {
        let input = input.trim();
        if !input.starts_with('/') {
            return None;
        }

        let rest = &input[1..];

        // If there's whitespace, we're past the command name
        if rest.contains(char::is_whitespace) {
            return None;
        }

        Some(rest)
    }

    /// Check if the cursor is in the command position (for completion).
    pub fn is_at_command_position(input: &str, cursor_pos: usize) -> bool {
        if !input.starts_with('/') {
            return false;
        }

        // Cursor is at command position if it's after / and before any whitespace
        let after_slash = &input[1..];
        let first_space = after_slash.find(char::is_whitespace);

        match first_space {
            Some(pos) => cursor_pos <= pos + 1, // +1 for the /
            None => true,                       // No space yet, so still at command
        }
    }

    /// Get the argument position at the cursor.
    ///
    /// Returns the 0-based index of which argument the cursor is in,
    /// or None if cursor is still in command name.
    pub fn get_argument_position(input: &str, cursor_pos: usize) -> Option<usize> {
        if !input.starts_with('/') {
            return None;
        }

        let input = &input[..cursor_pos.min(input.len())];

        // Find first space (end of command name)
        let first_space = input.find(char::is_whitespace)?;

        // Count arguments before cursor
        let args_portion = &input[first_space..];
        let parts: Vec<_> = args_portion.split_whitespace().collect();

        if parts.is_empty() {
            Some(0)
        } else {
            Some(parts.len() - 1)
        }
    }
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_command() {
        let cmd = CommandParser::parse("/quit").unwrap();
        assert_eq!(cmd.name, "quit");
        assert!(cmd.args.is_empty());
    }

    #[test]
    fn test_parse_command_with_whitespace() {
        let cmd = CommandParser::parse("  /quit  ").unwrap();
        assert_eq!(cmd.name, "quit");
    }

    #[test]
    fn test_parse_command_with_single_arg() {
        let cmd = CommandParser::parse("/help topic").unwrap();
        assert_eq!(cmd.name, "help");
        assert_eq!(cmd.args, vec!["topic"]);
    }

    #[test]
    fn test_parse_command_with_multiple_args() {
        let cmd = CommandParser::parse("/add file1.txt file2.txt").unwrap();
        assert_eq!(cmd.name, "add");
        assert_eq!(cmd.args, vec!["file1.txt", "file2.txt"]);
    }

    #[test]
    fn test_parse_command_with_quoted_arg() {
        let cmd = CommandParser::parse("/search \"hello world\"").unwrap();
        assert_eq!(cmd.name, "search");
        assert_eq!(cmd.args, vec!["hello world"]);
    }

    #[test]
    fn test_parse_command_with_single_quoted_arg() {
        let cmd = CommandParser::parse("/search 'hello world'").unwrap();
        assert_eq!(cmd.name, "search");
        assert_eq!(cmd.args, vec!["hello world"]);
    }

    #[test]
    fn test_parse_command_with_mixed_args() {
        let cmd = CommandParser::parse("/add file.txt \"path with spaces\" another").unwrap();
        assert_eq!(cmd.name, "add");
        assert_eq!(cmd.args, vec!["file.txt", "path with spaces", "another"]);
    }

    #[test]
    fn test_parse_command_case_insensitive() {
        let cmd = CommandParser::parse("/HELP").unwrap();
        assert_eq!(cmd.name, "help");
    }

    #[test]
    fn test_parse_invalid_not_command() {
        assert!(CommandParser::parse("hello").is_none());
        assert!(CommandParser::parse("").is_none());
    }

    #[test]
    fn test_parse_invalid_empty_command() {
        assert!(CommandParser::parse("/").is_none());
        assert!(CommandParser::parse("/  ").is_none());
    }

    #[test]
    fn test_parse_special_command() {
        let cmd = CommandParser::parse("/?").unwrap();
        assert_eq!(cmd.name, "?");
    }

    #[test]
    fn test_parse_hyphenated_command() {
        let cmd = CommandParser::parse("/mcp-tools").unwrap();
        assert_eq!(cmd.name, "mcp-tools");
    }

    #[test]
    fn test_parse_escaped_quotes() {
        let cmd = CommandParser::parse(r#"/echo "say \"hello\"""#).unwrap();
        assert_eq!(cmd.name, "echo");
        assert_eq!(cmd.args, vec!["say \"hello\""]);
    }

    #[test]
    fn test_is_command() {
        assert!(CommandParser::is_command("/help"));
        assert!(CommandParser::is_command("  /help  "));
        assert!(CommandParser::is_command("/?"));
        assert!(CommandParser::is_command("/h"));
        assert!(!CommandParser::is_command("help"));
        assert!(!CommandParser::is_command(""));
        assert!(!CommandParser::is_command("/"));
        assert!(!CommandParser::is_command("/ "));
    }

    #[test]
    fn test_extract_partial_command() {
        assert_eq!(CommandParser::extract_partial_command("/hel"), Some("hel"));
        assert_eq!(CommandParser::extract_partial_command("/"), Some(""));
        assert_eq!(CommandParser::extract_partial_command("/help topic"), None);
        assert_eq!(CommandParser::extract_partial_command("hello"), None);
    }

    #[test]
    fn test_is_at_command_position() {
        assert!(CommandParser::is_at_command_position("/help", 0));
        assert!(CommandParser::is_at_command_position("/help", 3));
        assert!(CommandParser::is_at_command_position("/help", 5));
        assert!(!CommandParser::is_at_command_position("/help topic", 7));
    }

    #[test]
    fn test_get_argument_position() {
        assert_eq!(CommandParser::get_argument_position("/help", 5), None);
        assert_eq!(CommandParser::get_argument_position("/help ", 6), Some(0));
        assert_eq!(
            CommandParser::get_argument_position("/help topic", 11),
            Some(0)
        );
        assert_eq!(
            CommandParser::get_argument_position("/help topic another", 19),
            Some(1)
        );
    }

    #[test]
    fn test_split_args_edge_cases() {
        // Empty
        let cmd = CommandParser::parse("/cmd").unwrap();
        assert!(cmd.args.is_empty());

        // Multiple spaces between args
        let cmd = CommandParser::parse("/cmd   arg1    arg2").unwrap();
        assert_eq!(cmd.args, vec!["arg1", "arg2"]);

        // Unclosed quote - should include everything
        let cmd = CommandParser::parse("/cmd \"unclosed").unwrap();
        assert_eq!(cmd.args, vec!["unclosed"]);
    }

    #[test]
    fn test_split_args_public_api() {
        // Test the public split_args function directly
        let args = CommandParser::split_args("file1.txt file2.txt");
        assert_eq!(args, vec!["file1.txt", "file2.txt"]);

        let args = CommandParser::split_args("\"my file.txt\" other.txt");
        assert_eq!(args, vec!["my file.txt", "other.txt"]);

        let args = CommandParser::split_args("'single quoted.txt' regular.txt");
        assert_eq!(args, vec!["single quoted.txt", "regular.txt"]);
    }

    #[test]
    fn test_split_args_filenames_with_spaces() {
        // This is the main bug fix test case for issue #409
        // Filenames with spaces should be parsed correctly when quoted
        let args = CommandParser::split_args("\"My Documents/file.txt\"");
        assert_eq!(args, vec!["My Documents/file.txt"]);

        let args = CommandParser::split_args("\"file one.txt\" \"file two.txt\"");
        assert_eq!(args, vec!["file one.txt", "file two.txt"]);

        // Mixed quoted and unquoted
        let args = CommandParser::split_args("simple.txt \"has spaces.txt\" another.txt");
        assert_eq!(args, vec!["simple.txt", "has spaces.txt", "another.txt"]);
    }

    #[test]
    fn test_args_quoted_roundtrip() {
        // Test that args_string_quoted produces output that split_args can parse back correctly
        use super::ParsedCommand;

        // Create a command with filenames containing spaces
        let original_args = vec![
            "my file.txt".to_string(),
            "other.txt".to_string(),
            "path with spaces/doc.pdf".to_string(),
        ];
        let cmd = ParsedCommand::new(
            "remove".to_string(),
            original_args.clone(),
            "/remove".to_string(),
        );

        // Get the quoted string representation
        let quoted = cmd.args_string_quoted();

        // Parse it back using split_args
        let parsed_args = CommandParser::split_args(&quoted);

        // Should match the original
        assert_eq!(parsed_args, original_args);
    }
}
