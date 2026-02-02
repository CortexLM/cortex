//! Tests for agent command functionality.

#[cfg(test)]
mod tests {
    use crate::agent_cmd::cli::{CopyArgs, ExportArgs};
    use crate::agent_cmd::loader::{
        load_builtin_agents, parse_frontmatter, read_file_with_encoding,
    };
    use crate::agent_cmd::types::AgentMode;

    #[test]
    fn test_read_file_with_utf8() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("test_utf8.md");

        // Create UTF-8 file
        let content = "---\nname: test\n---\n\nHello world!";
        std::fs::write(&test_file, content).unwrap();

        let result = read_file_with_encoding(&test_file);
        std::fs::remove_file(&test_file).ok();

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), content);
    }

    #[test]
    fn test_read_file_with_utf8_bom() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("test_utf8_bom.md");

        // Create UTF-8 file with BOM
        let content = "---\nname: test\n---";
        let mut bytes = vec![0xEF, 0xBB, 0xBF]; // UTF-8 BOM
        bytes.extend_from_slice(content.as_bytes());
        std::fs::write(&test_file, &bytes).unwrap();

        let result = read_file_with_encoding(&test_file);
        std::fs::remove_file(&test_file).ok();

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), content);
    }

    #[test]
    fn test_read_file_with_utf16_le() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("test_utf16_le.md");

        // Create UTF-16 LE file with BOM
        let content = "name: test";
        let mut bytes = vec![0xFF, 0xFE]; // UTF-16 LE BOM
        for c in content.encode_utf16() {
            bytes.extend_from_slice(&c.to_le_bytes());
        }
        std::fs::write(&test_file, &bytes).unwrap();

        let result = read_file_with_encoding(&test_file);
        std::fs::remove_file(&test_file).ok();

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), content);
    }

    #[test]
    fn test_read_file_with_utf16_be() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("test_utf16_be.md");

        // Create UTF-16 BE file with BOM
        let content = "name: test";
        let mut bytes = vec![0xFE, 0xFF]; // UTF-16 BE BOM
        for c in content.encode_utf16() {
            bytes.extend_from_slice(&c.to_be_bytes());
        }
        std::fs::write(&test_file, &bytes).unwrap();

        let result = read_file_with_encoding(&test_file);
        std::fs::remove_file(&test_file).ok();

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), content);
    }

    #[test]
    fn test_copy_args() {
        // Test that CopyArgs parses correctly
        let args = CopyArgs {
            source: "build".to_string(),
            destination: "my-build".to_string(),
            force: false,
        };
        assert_eq!(args.source, "build");
        assert_eq!(args.destination, "my-build");
    }

    #[test]
    fn test_export_args() {
        // Test that ExportArgs parses correctly
        let args = ExportArgs {
            name: "build".to_string(),
            output: None,
            json: false,
        };
        assert_eq!(args.name, "build");
        assert!(!args.json);
    }

    #[test]
    fn test_parse_frontmatter() {
        let content = r#"---
name: test-agent
description: A test agent
mode: primary
temperature: 0.5
---

This is the system prompt.
"#;

        let (frontmatter, body) = parse_frontmatter(content).unwrap();
        assert_eq!(frontmatter.name, "test-agent");
        assert_eq!(frontmatter.description, Some("A test agent".to_string()));
        assert!(matches!(frontmatter.mode, AgentMode::Primary));
        assert_eq!(frontmatter.temperature, Some(0.5));
        assert!(body.contains("system prompt"));
    }

    #[test]
    fn test_agent_mode_parsing() {
        assert!(matches!(
            "primary".parse::<AgentMode>().unwrap(),
            AgentMode::Primary
        ));
        assert!(matches!(
            "subagent".parse::<AgentMode>().unwrap(),
            AgentMode::Subagent
        ));
        assert!(matches!(
            "all".parse::<AgentMode>().unwrap(),
            AgentMode::All
        ));
        assert!("invalid".parse::<AgentMode>().is_err());
    }

    #[test]
    fn test_builtin_agents() {
        let agents = load_builtin_agents();
        assert!(!agents.is_empty());

        // Check for expected agents
        assert!(agents.iter().any(|a| a.name == "build"));
        assert!(agents.iter().any(|a| a.name == "plan"));
        assert!(agents.iter().any(|a| a.name == "explore"));
    }
}
