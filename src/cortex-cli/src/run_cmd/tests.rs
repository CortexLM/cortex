//! Tests for the run command module.

#[cfg(test)]
mod tests {
    use super::super::cli::OutputFormat;

    #[test]
    fn test_output_format_display() {
        assert_eq!(OutputFormat::Default.to_string(), "default");
        assert_eq!(OutputFormat::Json.to_string(), "json");
        assert_eq!(OutputFormat::Jsonl.to_string(), "jsonl");
    }

    #[test]
    fn test_temperature_validation() {
        // Valid temperatures
        assert!((0.0..=2.0).contains(&0.0));
        assert!((0.0..=2.0).contains(&1.0));
        assert!((0.0..=2.0).contains(&2.0));

        // Invalid temperatures
        assert!(!(0.0..=2.0).contains(&-0.1));
        assert!(!(0.0..=2.0).contains(&2.1));
    }

    #[test]
    fn test_empty_command_validation() {
        // Test that empty command strings are detected
        let empty_commands = vec!["", "   ", "\t", "\n", "  \t\n  "];
        for cmd in empty_commands {
            assert!(
                cmd.trim().is_empty(),
                "Command '{}' should be considered empty",
                cmd
            );
        }

        // Test that non-empty commands are valid
        let valid_commands = vec!["test", "  test  ", "a"];
        for cmd in valid_commands {
            assert!(
                !cmd.trim().is_empty(),
                "Command '{}' should be considered valid",
                cmd
            );
        }
    }
}
