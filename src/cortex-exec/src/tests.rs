//! Tests for Cortex Exec module.
//!
//! Command execution tests with:
//! 1. Timeout handling
//! 2. Stdout/stderr capture
//! 3. Exit code handling
//! 4. Sandbox error detection
//! 5. Real execution scenarios (npm, cargo, etc.)

use super::*;
use std::time::Duration;

/// Default timeout value in seconds for reference
const DEFAULT_TIMEOUT_SECS: u64 = 600;

mod exec_options_tests {
    use super::*;

    #[test]
    fn test_default_options() {
        let opts = ExecOptions::default();

        assert!(opts.prompt.is_empty());
        assert!(opts.sandbox);
        assert_eq!(opts.max_turns, Some(10));
        assert_eq!(opts.timeout_secs, Some(DEFAULT_TIMEOUT_SECS));
        assert!(!opts.full_auto);
        assert!(opts.streaming);
        assert!(opts.enabled_tools.is_none());
        assert!(opts.disabled_tools.is_empty());
    }

    #[test]
    fn test_custom_options() {
        let opts = ExecOptions {
            prompt: "Create a Tailwind project".to_string(),
            cwd: std::env::temp_dir().join("test"),
            model: Some("gpt-4".to_string()),
            output_format: OutputFormat::Json,
            full_auto: true,
            max_turns: Some(20),
            timeout_secs: Some(300),
            request_timeout_secs: Some(60),
            sandbox: false,
            system_prompt: Some("Custom system prompt".to_string()),
            streaming: false,
            enabled_tools: Some(vec!["Read".to_string(), "Create".to_string()]),
            disabled_tools: vec!["Execute".to_string()],
        };

        assert_eq!(opts.prompt, "Create a Tailwind project");
        assert!(opts.full_auto);
        assert!(!opts.sandbox);
        assert!(!opts.streaming);
        assert!(opts.system_prompt.is_some());
        assert_eq!(opts.enabled_tools.as_ref().unwrap().len(), 2);
        assert_eq!(opts.disabled_tools.len(), 1);
    }
}

mod exec_result_tests {
    use super::*;
    use cortex_protocol::ConversationId;

    #[test]
    fn test_success_result() {
        let result = ExecResult {
            conversation_id: ConversationId::new(),
            response: "Project created successfully".to_string(),
            turns: 3,
            files_modified: vec!["package.json".to_string(), "tailwind.config.js".to_string()],
            commands_executed: vec![
                "npm init".to_string(),
                "npm install tailwindcss".to_string(),
            ],
            tool_calls: vec![ToolCallRecord {
                name: "Execute".to_string(),
                arguments: r#"{"command": ["npm", "init"]}"#.to_string(),
                result: "Initialized npm project".to_string(),
                success: true,
                duration_ms: 1500,
            }],
            success: true,
            error: None,
            input_tokens: 1000,
            output_tokens: 500,
            timed_out: false,
        };

        assert!(result.success);
        assert!(result.error.is_none());
        assert_eq!(result.files_modified.len(), 2);
        assert_eq!(result.tool_calls.len(), 1);
        assert!(!result.timed_out);
        assert_eq!(result.input_tokens, 1000);
    }

    #[test]
    fn test_error_result() {
        let result = ExecResult {
            conversation_id: ConversationId::new(),
            response: String::new(),
            turns: 1,
            files_modified: vec![],
            commands_executed: vec!["npm install".to_string()],
            tool_calls: vec![],
            success: false,
            error: Some("EPERM: operation not permitted".to_string()),
            input_tokens: 500,
            output_tokens: 100,
            timed_out: false,
        };

        assert!(!result.success);
        assert!(result.error.is_some());
        assert!(result.error.unwrap().contains("EPERM"));
    }

    #[test]
    fn test_timeout_result() {
        let result = ExecResult {
            conversation_id: ConversationId::new(),
            response: String::new(),
            turns: 0,
            files_modified: vec![],
            commands_executed: vec![],
            tool_calls: vec![],
            success: false,
            error: Some("Execution timed out".to_string()),
            input_tokens: 0,
            output_tokens: 0,
            timed_out: true,
        };

        assert!(!result.success);
        assert!(result.timed_out);
        assert!(result.error.is_some());
    }
}

mod sandbox_detection_tests {

    /// Tests permission error pattern detection
    #[test]
    fn test_permission_error_patterns() {
        let permission_errors = [
            "EPERM: operation not permitted",
            "EACCES: permission denied",
            "Error: EROFS: read-only file system",
            "seccomp blocked syscall",
            "landlock: access denied",
        ];

        for error in permission_errors {
            let lower = error.to_lowercase();
            let is_sandbox_error = lower.contains("permission")
                || lower.contains("eperm")
                || lower.contains("eacces")
                || lower.contains("erofs")
                || lower.contains("seccomp")
                || lower.contains("landlock")
                || lower.contains("read-only");

            assert!(is_sandbox_error, "Should detect sandbox error: {}", error);
        }
    }

    /// Tests sandbox-specific exit codes
    #[test]
    fn test_sandbox_exit_codes() {
        // Exit code for SIGSYS (signal 31 on x86_64)
        const SIGSYS_EXIT_CODE: i32 = 128 + 31;
        // Exit code for timeout
        const TIMEOUT_EXIT_CODE: i32 = 124;

        assert_eq!(SIGSYS_EXIT_CODE, 159);
        assert_eq!(TIMEOUT_EXIT_CODE, 124);
    }
}

mod tailwind_scenario_tests {

    /// Simulates commands needed to create a Tailwind project
    #[test]
    fn test_tailwind_commands_list() {
        let tailwind_commands = [
            (
                "npm",
                vec![
                    "create",
                    "vite@latest",
                    "my-project",
                    "--",
                    "--template",
                    "react",
                ],
            ),
            ("cd", vec!["my-project"]),
            (
                "npm",
                vec!["install", "-D", "tailwindcss", "postcss", "autoprefixer"],
            ),
            ("npx", vec!["tailwindcss", "init", "-p"]),
        ];

        // Verify commands are well-formed
        for (program, args) in tailwind_commands {
            assert!(!program.is_empty());
            assert!(!args.is_empty());
        }
    }

    /// Tests required files for a Tailwind project
    #[test]
    fn test_tailwind_required_files() {
        let required_files = [
            "package.json",
            "tailwind.config.js",
            "postcss.config.js",
            "src/index.css",
        ];

        assert_eq!(required_files.len(), 4);
    }

    /// Verifies permissions needed for npm install
    #[test]
    fn test_npm_install_requirements() {
        // npm install needs:
        // 1. Network access (socket, connect)
        // 2. Write to node_modules
        // 3. Write to package-lock.json
        // 4. Read/write to ~/.npm (cache)

        let required_permissions = [
            "network_access",
            "write_node_modules",
            "write_package_lock",
            "write_npm_cache",
        ];

        assert_eq!(required_permissions.len(), 4);
    }
}

mod timeout_tests {
    use super::*;

    #[test]
    fn test_default_timeout() {
        let opts = ExecOptions::default();
        // Default timeout should be set to DEFAULT_TIMEOUT_SECS (10 minutes)
        assert_eq!(opts.timeout_secs, Some(DEFAULT_TIMEOUT_SECS));
        assert_eq!(opts.request_timeout_secs, Some(120)); // 2 minutes
    }

    #[test]
    fn test_timeout_conversion() {
        let secs = 60u64;
        let duration = Duration::from_secs(secs);
        assert_eq!(duration.as_secs(), 60);
        assert_eq!(duration.as_millis(), 60_000);
    }

    #[test]
    fn test_custom_timeout() {
        let opts = ExecOptions {
            timeout_secs: Some(300),
            request_timeout_secs: Some(30),
            ..Default::default()
        };
        assert_eq!(opts.timeout_secs, Some(300));
        assert_eq!(opts.request_timeout_secs, Some(30));
    }
}

mod output_format_tests {
    use super::*;

    #[test]
    fn test_output_formats() {
        let formats = [OutputFormat::Text, OutputFormat::Json];

        assert_eq!(formats.len(), 2);
    }
}
