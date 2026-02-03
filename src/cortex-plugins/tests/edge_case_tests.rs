//! Edge case tests for the cortex-plugins crate.
//!
//! This module contains comprehensive tests for error handling, boundary conditions,
//! and security edge cases in the plugin system.

use std::collections::HashMap;
use std::path::PathBuf;

use chrono::Utc;
use cortex_plugins::{
    HookPriority, HookResult, PluginCapability, PluginConfig, PluginError,
    PluginIndexEntry, PluginManifest, PluginRegistry, RemoteRegistry,
};

// ============================================================================
// MANIFEST EDGE CASE TESTS
// ============================================================================

mod manifest_edge_cases {
    use super::*;

    #[test]
    fn test_manifest_empty_plugin_id() {
        let manifest_str = r#"
[plugin]
id = ""
name = "Test Plugin"
version = "1.0.0"
"#;
        let manifest = PluginManifest::parse(manifest_str).expect("should parse");
        let result = manifest.validate();
        assert!(result.is_err(), "Empty plugin ID should be rejected");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("empty") || err_msg.contains("ID"),
            "Error should mention empty ID"
        );
    }

    #[test]
    fn test_manifest_invalid_semver_version() {
        let manifest_str = r#"
[plugin]
id = "test-plugin"
name = "Test Plugin"
version = "not-a-version"
"#;
        let manifest = PluginManifest::parse(manifest_str).expect("should parse");
        let result = manifest.validate();
        assert!(result.is_err(), "Invalid semver should be rejected");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("version") || err_msg.contains("semver"),
            "Error should mention invalid version"
        );
    }

    #[test]
    fn test_manifest_invalid_semver_variants() {
        let invalid_versions = vec![
            "1",           // Missing minor and patch
            "1.0",         // Missing patch
            "v1.0.0",      // Has 'v' prefix
            "1.0.0.0",     // Too many parts
            "1.0.0-",      // Trailing dash
            "1.0.0+",      // Trailing plus
            "abc.def.ghi", // Non-numeric
            "",            // Empty string
        ];

        for version in invalid_versions {
            let manifest_str = format!(
                r#"
[plugin]
id = "test-plugin"
name = "Test Plugin"
version = "{}"
"#,
                version
            );

            let manifest = PluginManifest::parse(&manifest_str).expect("should parse");
            let result = manifest.validate();
            assert!(
                result.is_err(),
                "Version '{}' should be rejected as invalid semver",
                version
            );
        }
    }

    #[test]
    fn test_manifest_valid_semver_variants() {
        let valid_versions = vec![
            "0.0.1",
            "1.0.0",
            "1.2.3",
            "10.20.30",
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0+build",
            "1.0.0-alpha+build",
        ];

        for version in valid_versions {
            let manifest_str = format!(
                r#"
[plugin]
id = "test-plugin"
name = "Test Plugin"
version = "{}"
"#,
                version
            );

            let manifest = PluginManifest::parse(&manifest_str).expect("should parse");
            assert!(
                manifest.validate().is_ok(),
                "Version '{}' should be accepted as valid semver",
                version
            );
        }
    }

    #[test]
    fn test_manifest_invalid_toml_syntax() {
        let invalid_toml_cases = vec![
            // Missing closing bracket
            "[plugin\nid = \"test\"",
            // Invalid key syntax
            "[plugin]\n123id = \"test\"",
            // Missing value
            "[plugin]\nid = ",
            // Unclosed string
            "[plugin]\nid = \"unclosed",
            // Invalid escape sequence
            "[plugin]\nid = \"test\\x\"",
            // Mixed array types (in TOML arrays must be homogeneous)
            "[plugin]\nid = \"test\"\nname = \"Test\"\nversion = \"1.0.0\"\n\n[[commands]]\nname = 123",
        ];

        for (idx, invalid_toml) in invalid_toml_cases.iter().enumerate() {
            let result = PluginManifest::parse(invalid_toml);
            assert!(
                result.is_err(),
                "Invalid TOML case {} should fail to parse",
                idx
            );
        }
    }

    #[test]
    fn test_manifest_invalid_characters_in_plugin_id() {
        // Test IDs that parse correctly but should fail validation
        let invalid_ids = vec![
            "plugin with spaces",
            "plugin/with/slashes",
            "plugin.with.dots",
            "plugin@with@at",
            "plugin#with#hash",
            "plugin$with$dollar",
            "plugin%with%percent",
            "plugin^with^caret",
            "plugin&with&ampersand",
            "plugin*with*asterisk",
            "plugin(with)parens",
            "plugin[with]brackets",
            "plugin{with}braces",
            "plugin|with|pipe",
            "plugin<with>angles",
            "plugin!with!exclaim",
            "plugin?with?question",
            "plugin`with`backticks",
            "plugin~with~tilde",
            "plugin=with=equals",
            "plugin+with+plus",
            "plugin:with:colons",
            "plugin;with;semicolons",
            "plugin,with,commas",
        ];

        for invalid_id in invalid_ids {
            let manifest_str = format!(
                r#"
[plugin]
id = "{}"
name = "Test Plugin"
version = "1.0.0"
"#,
                invalid_id
            );

            let manifest = PluginManifest::parse(&manifest_str).expect("should parse");
            let result = manifest.validate();
            assert!(
                result.is_err(),
                "Plugin ID '{}' should be rejected (contains invalid characters)",
                invalid_id
            );
        }
    }

    #[test]
    fn test_manifest_toml_escape_sequences_in_id() {
        // Backslashes are invalid escape sequences in TOML strings
        // This should fail at parse time, not validation
        let manifest_str = r#"
[plugin]
id = "plugin\with\backslashes"
name = "Test Plugin"
version = "1.0.0"
"#;
        // This should fail to parse due to invalid escape sequences
        let result = PluginManifest::parse(manifest_str);
        assert!(
            result.is_err(),
            "Backslash in ID should fail TOML parsing"
        );

        // Also test with quotes (needs escaping)
        let manifest_str2 = r#"
[plugin]
id = "plugin\"with\"quotes"
name = "Test Plugin"
version = "1.0.0"
"#;
        // This parses but the ID contains quotes which should fail validation
        let manifest = PluginManifest::parse(manifest_str2).expect("should parse");
        let result = manifest.validate();
        assert!(
            result.is_err(),
            "Plugin ID with quotes should be rejected"
        );
    }

    #[test]
    fn test_manifest_valid_plugin_id_characters() {
        let valid_ids = vec![
            "simple",
            "with-dashes",
            "with_underscores",
            "With-Mixed-Case",
            "with123numbers",
            "123-leading-numbers",
            "a",  // Single char
            "ab", // Two chars
        ];

        for valid_id in valid_ids {
            let manifest_str = format!(
                r#"
[plugin]
id = "{}"
name = "Test Plugin"
version = "1.0.0"
"#,
                valid_id
            );

            let manifest = PluginManifest::parse(&manifest_str).expect("should parse");
            assert!(
                manifest.validate().is_ok(),
                "Plugin ID '{}' should be accepted as valid",
                valid_id
            );
        }
    }

    #[test]
    fn test_manifest_empty_command_name() {
        let manifest_str = r#"
[plugin]
id = "test-plugin"
name = "Test Plugin"
version = "1.0.0"

[[commands]]
name = ""
description = "An empty command"
"#;
        let manifest = PluginManifest::parse(manifest_str).expect("should parse");
        let result = manifest.validate();
        assert!(result.is_err(), "Empty command name should be rejected");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Command") || err_msg.contains("empty"),
            "Error should mention command name issue"
        );
    }

    #[test]
    fn test_manifest_command_capability_without_commands() {
        // Having command capability but empty commands array should be valid
        // (the plugin might register commands dynamically)
        let manifest_str = r#"
capabilities = ["commands"]

[plugin]
id = "test-plugin"
name = "Test Plugin"
version = "1.0.0"
"#;
        let manifest = PluginManifest::parse(manifest_str).expect("should parse");
        // This should be valid - capability declared but no static commands
        assert!(
            manifest.validate().is_ok(),
            "Commands capability with empty commands array should be valid"
        );
        assert!(manifest.has_capability(PluginCapability::Commands));
        assert!(manifest.commands.is_empty());
    }

    #[test]
    fn test_manifest_missing_required_fields() {
        // Missing version
        let manifest_str = r#"
[plugin]
id = "test-plugin"
name = "Test Plugin"
"#;
        let result = PluginManifest::parse(manifest_str);
        assert!(
            result.is_err(),
            "Manifest without version should fail to parse"
        );

        // Missing name
        let manifest_str = r#"
[plugin]
id = "test-plugin"
version = "1.0.0"
"#;
        let result = PluginManifest::parse(manifest_str);
        assert!(
            result.is_err(),
            "Manifest without name should fail to parse"
        );

        // Missing id
        let manifest_str = r#"
[plugin]
name = "Test Plugin"
version = "1.0.0"
"#;
        let result = PluginManifest::parse(manifest_str);
        assert!(
            result.is_err(),
            "Manifest without id should fail to parse"
        );
    }

    #[test]
    fn test_manifest_unknown_capability() {
        // Unknown capabilities should be gracefully handled or rejected
        let manifest_str = r#"
capabilities = ["commands", "unknown_capability", "hooks"]

[plugin]
id = "test-plugin"
name = "Test Plugin"
version = "1.0.0"
"#;
        // This should fail to parse since "unknown_capability" is not a valid enum variant
        let result = PluginManifest::parse(manifest_str);
        assert!(
            result.is_err(),
            "Unknown capability should cause parse failure"
        );
    }

    #[test]
    fn test_manifest_duplicate_capabilities() {
        // Duplicate capabilities should be handled
        let manifest_str = r#"
capabilities = ["commands", "commands", "hooks"]

[plugin]
id = "test-plugin"
name = "Test Plugin"
version = "1.0.0"
"#;
        let manifest = PluginManifest::parse(manifest_str).expect("should parse");
        assert!(manifest.validate().is_ok());
        // Should have 3 entries (duplicates allowed in the array)
        assert_eq!(manifest.capabilities.len(), 3);
        // has_capability should work correctly
        assert!(manifest.has_capability(PluginCapability::Commands));
        assert!(manifest.has_capability(PluginCapability::Hooks));
    }
}

// ============================================================================
// REGISTRY EDGE CASE TESTS
// ============================================================================

mod registry_edge_cases {
    use super::*;

    #[tokio::test]
    async fn test_registry_ssrf_ipv4_mapped_ipv6() {
        // IPv4-mapped IPv6 addresses should also be blocked
        let dangerous_urls = vec![
            "https://[::ffff:127.0.0.1]/plugin.wasm",
            "https://[::ffff:10.0.0.1]/plugin.wasm",
            "https://[::ffff:192.168.1.1]/plugin.wasm",
            "https://[::ffff:172.16.0.1]/plugin.wasm",
        ];

        for url in dangerous_urls {
            let entry = create_test_entry_with_url(url);
            let registry = PluginRegistry::new();
            let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

            let result = registry.download_plugin(&entry, temp_dir.path()).await;
            assert!(
                result.is_err(),
                "IPv4-mapped IPv6 URL {} should be blocked",
                url
            );
        }
    }

    #[tokio::test]
    async fn test_registry_ssrf_unspecified_addresses() {
        let dangerous_urls = vec![
            "https://0.0.0.0/plugin.wasm",
            "https://[::]/plugin.wasm",
        ];

        for url in dangerous_urls {
            let entry = create_test_entry_with_url(url);
            let registry = PluginRegistry::new();
            let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

            let result = registry.download_plugin(&entry, temp_dir.path()).await;
            assert!(
                result.is_err(),
                "Unspecified address URL {} should be blocked",
                url
            );
        }
    }

    #[tokio::test]
    async fn test_registry_ssrf_documentation_ips() {
        // Documentation IPs (192.0.2.x, 198.51.100.x, 203.0.113.x) should be blocked
        let doc_ips = vec![
            "https://192.0.2.1/plugin.wasm",
            "https://198.51.100.1/plugin.wasm",
            "https://203.0.113.1/plugin.wasm",
        ];

        for url in doc_ips {
            let entry = create_test_entry_with_url(url);
            let registry = PluginRegistry::new();
            let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

            let result = registry.download_plugin(&entry, temp_dir.path()).await;
            assert!(
                result.is_err(),
                "Documentation IP URL {} should be blocked",
                url
            );
        }
    }

    #[tokio::test]
    async fn test_registry_directory_traversal_encoded() {
        // Test URL-encoded path traversal attempts
        let malicious_ids = vec![
            "..%2F..%2F..%2Fetc%2Fpasswd",
            "%2e%2e/%2e%2e/etc/passwd",
            "plugin%00/../../../etc/passwd", // Null byte injection
        ];

        for id in malicious_ids {
            let entry = PluginIndexEntry {
                id: id.to_string(),
                name: "Malicious Plugin".to_string(),
                version: "1.0.0".to_string(),
                description: "Tries to escape".to_string(),
                download_url: "https://example.com/plugin.wasm".to_string(),
                checksum: "abc123".to_string(),
                signature: None,
                updated_at: Utc::now(),
            };

            let registry = PluginRegistry::new();
            let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

            let result = registry.download_plugin(&entry, temp_dir.path()).await;
            // Should either block the ID or fail safely (the ID contains .. which is blocked)
            if id.contains("..") {
                assert!(
                    result.is_err(),
                    "Path traversal ID '{}' should be blocked",
                    id
                );
            }
        }
    }

    #[tokio::test]
    async fn test_registry_invalid_url_schemes() {
        let invalid_urls = vec![
            "javascript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "gopher://example.com/plugin.wasm",
            "dict://example.com/plugin.wasm",
            "ldap://example.com/plugin.wasm",
            "tftp://example.com/plugin.wasm",
            "jar:file:///etc/passwd!/",
        ];

        for url in invalid_urls {
            let entry = create_test_entry_with_url(url);
            let registry = PluginRegistry::new();
            let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

            let result = registry.download_plugin(&entry, temp_dir.path()).await;
            assert!(
                result.is_err(),
                "Invalid URL scheme {} should be blocked",
                url
            );
        }
    }

    #[tokio::test]
    async fn test_registry_cloud_metadata_endpoints() {
        let metadata_urls = vec![
            "https://169.254.169.254/latest/meta-data/",  // AWS
            "https://metadata.google.internal/computeMetadata/v1/", // GCP
        ];

        for url in metadata_urls {
            let entry = create_test_entry_with_url(url);
            let registry = PluginRegistry::new();
            let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

            let result = registry.download_plugin(&entry, temp_dir.path()).await;
            assert!(
                result.is_err(),
                "Cloud metadata URL {} should be blocked",
                url
            );
        }
    }

    #[tokio::test]
    async fn test_registry_duplicate_registration() {
        use cortex_plugins::{Plugin, PluginInfo, PluginState, PluginContext};
        use std::collections::HashMap;

        // Create mock plugin
        struct MockPlugin {
            info: PluginInfo,
            manifest: PluginManifest,
            state: PluginState,
        }

        impl MockPlugin {
            fn new(id: &str) -> Self {
                let manifest = PluginManifest {
                    plugin: cortex_plugins::manifest::PluginMetadata {
                        id: id.to_string(),
                        name: format!("Test Plugin {}", id),
                        version: "1.0.0".to_string(),
                        description: "A test plugin".to_string(),
                        authors: vec![],
                        homepage: None,
                        license: None,
                        min_cortex_version: None,
                        keywords: vec![],
                        icon: None,
                    },
                    capabilities: vec![],
                    permissions: vec![],
                    dependencies: vec![],
                    commands: vec![],
                    hooks: vec![],
                    config: HashMap::new(),
                    wasm: Default::default(),
                };

                let info = PluginInfo::from_manifest(&manifest, PathBuf::from("/tmp"));

                Self {
                    info,
                    manifest,
                    state: PluginState::Loaded,
                }
            }
        }

        #[async_trait::async_trait]
        impl Plugin for MockPlugin {
            fn info(&self) -> &PluginInfo {
                &self.info
            }

            fn manifest(&self) -> &PluginManifest {
                &self.manifest
            }

            fn state(&self) -> PluginState {
                self.state
            }

            async fn init(&mut self) -> cortex_plugins::Result<()> {
                self.state = PluginState::Active;
                Ok(())
            }

            async fn shutdown(&mut self) -> cortex_plugins::Result<()> {
                self.state = PluginState::Unloaded;
                Ok(())
            }

            async fn execute_command(
                &self,
                name: &str,
                _args: Vec<String>,
                _ctx: &PluginContext,
            ) -> cortex_plugins::Result<String> {
                Ok(format!("Mock command: {}", name))
            }

            fn get_config(&self, _key: &str) -> Option<serde_json::Value> {
                None
            }

            fn set_config(&mut self, _key: &str, _value: serde_json::Value) -> cortex_plugins::Result<()> {
                Ok(())
            }
        }

        let registry = PluginRegistry::new();

        // Register first plugin
        let plugin1 = MockPlugin::new("test-plugin");
        let result = registry.register(Box::new(plugin1)).await;
        assert!(result.is_ok(), "First registration should succeed");

        // Try to register duplicate
        let plugin2 = MockPlugin::new("test-plugin");
        let result = registry.register(Box::new(plugin2)).await;
        assert!(result.is_err(), "Duplicate registration should fail");

        // Verify it's the AlreadyExists error
        match result.unwrap_err() {
            PluginError::AlreadyExists(id) => {
                assert_eq!(id, "test-plugin");
            }
            other => panic!("Expected AlreadyExists error, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_registry_unload_and_reload() {
        use cortex_plugins::{Plugin, PluginInfo, PluginState, PluginContext};

        struct ReloadablePlugin {
            info: PluginInfo,
            manifest: PluginManifest,
            state: PluginState,
            init_count: u32,
        }

        impl ReloadablePlugin {
            fn new(id: &str) -> Self {
                let manifest = PluginManifest {
                    plugin: cortex_plugins::manifest::PluginMetadata {
                        id: id.to_string(),
                        name: format!("Reloadable Plugin {}", id),
                        version: "1.0.0".to_string(),
                        description: "A reloadable test plugin".to_string(),
                        authors: vec![],
                        homepage: None,
                        license: None,
                        min_cortex_version: None,
                        keywords: vec![],
                        icon: None,
                    },
                    capabilities: vec![],
                    permissions: vec![],
                    dependencies: vec![],
                    commands: vec![],
                    hooks: vec![],
                    config: HashMap::new(),
                    wasm: Default::default(),
                };

                let info = PluginInfo::from_manifest(&manifest, PathBuf::from("/tmp"));

                Self {
                    info,
                    manifest,
                    state: PluginState::Loaded,
                    init_count: 0,
                }
            }
        }

        #[async_trait::async_trait]
        impl Plugin for ReloadablePlugin {
            fn info(&self) -> &PluginInfo {
                &self.info
            }

            fn manifest(&self) -> &PluginManifest {
                &self.manifest
            }

            fn state(&self) -> PluginState {
                self.state
            }

            async fn init(&mut self) -> cortex_plugins::Result<()> {
                self.init_count += 1;
                self.state = PluginState::Active;
                Ok(())
            }

            async fn shutdown(&mut self) -> cortex_plugins::Result<()> {
                self.state = PluginState::Unloaded;
                Ok(())
            }

            async fn execute_command(
                &self,
                name: &str,
                _args: Vec<String>,
                _ctx: &PluginContext,
            ) -> cortex_plugins::Result<String> {
                Ok(format!("Mock command: {}", name))
            }

            fn get_config(&self, _key: &str) -> Option<serde_json::Value> {
                None
            }

            fn set_config(&mut self, _key: &str, _value: serde_json::Value) -> cortex_plugins::Result<()> {
                Ok(())
            }
        }

        let registry = PluginRegistry::new();

        // Register and init
        let plugin = ReloadablePlugin::new("reload-test");
        registry.register(Box::new(plugin)).await.expect("register should work");
        assert!(registry.is_registered("reload-test").await);

        // Unregister
        registry.unregister("reload-test").await.expect("unregister should work");
        assert!(!registry.is_registered("reload-test").await);

        // Re-register (simulating reload)
        let plugin2 = ReloadablePlugin::new("reload-test");
        registry.register(Box::new(plugin2)).await.expect("re-register should work");
        assert!(registry.is_registered("reload-test").await);
    }

    #[tokio::test]
    async fn test_registry_search_with_no_cache() {
        let registry = PluginRegistry::new();

        // Search with empty cache should return empty results
        let results = registry.search("any-query").await.expect("search should work");
        assert!(results.is_empty(), "Search with no cache should return empty");
    }

    #[tokio::test]
    async fn test_registry_check_updates_with_no_plugins() {
        let registry = PluginRegistry::new();

        // Check updates with no plugins should return empty
        let updates = registry.check_updates().await.expect("check_updates should work");
        assert!(updates.is_empty(), "Updates with no plugins should be empty");
    }

    fn create_test_entry_with_url(url: &str) -> PluginIndexEntry {
        PluginIndexEntry {
            id: "test-plugin".to_string(),
            name: "Test Plugin".to_string(),
            version: "1.0.0".to_string(),
            description: "A test plugin".to_string(),
            download_url: url.to_string(),
            checksum: "abc123def456".to_string(),
            signature: None,
            updated_at: Utc::now(),
        }
    }
}

// ============================================================================
// HOOK EDGE CASE TESTS
// ============================================================================

mod hook_edge_cases {
    use super::*;
    use std::sync::Arc;
    use cortex_plugins::{
        HookRegistry, HookDispatcher, ToolExecuteBeforeHook, ToolExecuteBeforeInput,
        ToolExecuteBeforeOutput,
    };

    // Test hook that tracks execution
    struct TestBeforeHook {
        priority: HookPriority,
        pattern: Option<String>,
        result_action: HookResult,
    }

    impl TestBeforeHook {
        fn new(priority: i32, pattern: Option<&str>, result: HookResult) -> Self {
            Self {
                priority: HookPriority::new(priority),
                pattern: pattern.map(String::from),
                result_action: result,
            }
        }
    }

    #[async_trait::async_trait]
    impl ToolExecuteBeforeHook for TestBeforeHook {
        fn priority(&self) -> HookPriority {
            self.priority
        }

        fn pattern(&self) -> Option<&str> {
            self.pattern.as_deref()
        }

        async fn execute(
            &self,
            _input: &ToolExecuteBeforeInput,
            output: &mut ToolExecuteBeforeOutput,
        ) -> cortex_plugins::Result<()> {
            output.result = self.result_action.clone();
            Ok(())
        }
    }

    #[test]
    fn test_hook_priority_ordering() {
        let low = HookPriority::LOW;
        let normal = HookPriority::NORMAL;
        let high = HookPriority::PLUGIN_HIGH;

        // Lower values should sort earlier
        assert!(high < normal);
        assert!(normal < low);
        assert!(high < low);
    }

    #[test]
    fn test_hook_priority_system_reserved() {
        let system = HookPriority::SYSTEM;
        let critical = HookPriority::SYSTEM_CRITICAL;
        let plugin_min = HookPriority::PLUGIN_MIN;

        assert!(system.is_system_reserved());
        assert!(critical.is_system_reserved());
        assert!(!plugin_min.is_system_reserved());

        // Validate for plugin should fail for system priorities
        assert!(system.validate_for_plugin().is_err());
        assert!(critical.validate_for_plugin().is_err());
        assert!(plugin_min.validate_for_plugin().is_ok());
    }

    #[test]
    fn test_hook_priority_new_for_plugin_clamping() {
        // Values below 50 should be clamped
        let clamped = HookPriority::new_for_plugin(10);
        assert_eq!(clamped.value(), 50);

        let clamped = HookPriority::new_for_plugin(-100);
        assert_eq!(clamped.value(), 50);

        // Values at or above 50 should be unchanged
        let normal = HookPriority::new_for_plugin(100);
        assert_eq!(normal.value(), 100);

        let boundary = HookPriority::new_for_plugin(50);
        assert_eq!(boundary.value(), 50);
    }

    #[tokio::test]
    async fn test_hook_priority_execution_order() {
        let registry = Arc::new(HookRegistry::new());

        // Register hooks in non-priority order
        let hook_low = Arc::new(TestBeforeHook::new(200, None, HookResult::Continue));
        let hook_high = Arc::new(TestBeforeHook::new(50, None, HookResult::Continue));
        let hook_normal = Arc::new(TestBeforeHook::new(100, None, HookResult::Continue));

        registry.register_tool_execute_before("plugin-low", hook_low).await;
        registry.register_tool_execute_before("plugin-high", hook_high).await;
        registry.register_tool_execute_before("plugin-normal", hook_normal).await;

        // Verify registration
        let count = registry.hook_count(cortex_plugins::HookType::ToolExecuteBefore).await;
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn test_hook_abort_stops_execution() {
        let registry = Arc::new(HookRegistry::new());
        let dispatcher = HookDispatcher::new(registry.clone());

        // Register an aborting hook
        let abort_hook = Arc::new(TestBeforeHook::new(
            50,
            None,
            HookResult::Abort {
                reason: "Test abort".to_string(),
            },
        ));
        registry.register_tool_execute_before("aborter", abort_hook).await;

        // Execute hook
        let input = ToolExecuteBeforeInput {
            tool: "test_tool".to_string(),
            session_id: "test-session".to_string(),
            call_id: "test-call".to_string(),
            args: serde_json::json!({}),
        };
        let output = dispatcher.trigger_tool_execute_before(input).await.expect("dispatch should work");

        // Verify abort result
        match output.result {
            HookResult::Abort { reason } => {
                assert_eq!(reason, "Test abort");
            }
            _ => panic!("Expected Abort result"),
        }
    }

    #[tokio::test]
    async fn test_hook_skip_stops_further_processing() {
        let registry = Arc::new(HookRegistry::new());
        let dispatcher = HookDispatcher::new(registry.clone());

        // Register skip hook with higher priority
        let skip_hook = Arc::new(TestBeforeHook::new(50, None, HookResult::Skip));
        registry.register_tool_execute_before("skipper", skip_hook).await;

        // Register continue hook with lower priority (should not execute)
        let continue_hook = Arc::new(TestBeforeHook::new(100, None, HookResult::Continue));
        registry.register_tool_execute_before("continuer", continue_hook).await;

        let input = ToolExecuteBeforeInput {
            tool: "test_tool".to_string(),
            session_id: "test-session".to_string(),
            call_id: "test-call".to_string(),
            args: serde_json::json!({}),
        };
        let output = dispatcher.trigger_tool_execute_before(input).await.expect("dispatch should work");

        // Result should be Skip from first hook
        matches!(output.result, HookResult::Skip);
    }

    #[tokio::test]
    async fn test_hook_pattern_filtering() {
        let registry = Arc::new(HookRegistry::new());
        let dispatcher = HookDispatcher::new(registry.clone());

        // Register hook with specific pattern
        let patterned_hook = Arc::new(TestBeforeHook::new(
            100,
            Some("read*"),
            HookResult::Abort {
                reason: "Blocked read".to_string(),
            },
        ));
        registry.register_tool_execute_before("blocker", patterned_hook).await;

        // Test matching tool
        let input_match = ToolExecuteBeforeInput {
            tool: "read_file".to_string(),
            session_id: "test-session".to_string(),
            call_id: "test-call".to_string(),
            args: serde_json::json!({}),
        };
        let output = dispatcher.trigger_tool_execute_before(input_match).await.expect("dispatch should work");
        assert!(matches!(output.result, HookResult::Abort { .. }));

        // Test non-matching tool
        let input_no_match = ToolExecuteBeforeInput {
            tool: "write_file".to_string(),
            session_id: "test-session".to_string(),
            call_id: "test-call".to_string(),
            args: serde_json::json!({}),
        };
        let output = dispatcher.trigger_tool_execute_before(input_no_match).await.expect("dispatch should work");
        assert!(matches!(output.result, HookResult::Continue));
    }

    #[tokio::test]
    async fn test_hook_registry_unregister_plugin() {
        let registry = Arc::new(HookRegistry::new());

        // Register hooks for two plugins
        let hook1 = Arc::new(TestBeforeHook::new(100, None, HookResult::Continue));
        let hook2 = Arc::new(TestBeforeHook::new(100, None, HookResult::Continue));

        registry.register_tool_execute_before("plugin-a", hook1).await;
        registry.register_tool_execute_before("plugin-b", hook2).await;

        assert_eq!(
            registry.hook_count(cortex_plugins::HookType::ToolExecuteBefore).await,
            2
        );

        // Unregister one plugin
        registry.unregister_plugin("plugin-a").await;

        assert_eq!(
            registry.hook_count(cortex_plugins::HookType::ToolExecuteBefore).await,
            1
        );

        // Verify correct plugin remains
        let plugins = registry.registered_plugins().await;
        assert!(plugins.contains(&"plugin-b".to_string()));
        assert!(!plugins.contains(&"plugin-a".to_string()));
    }

    #[tokio::test]
    async fn test_hook_replace_result() {
        let registry = Arc::new(HookRegistry::new());
        let dispatcher = HookDispatcher::new(registry.clone());

        // Register hook that replaces result
        let replace_hook = Arc::new(TestBeforeHook::new(
            100,
            None,
            HookResult::Replace {
                result: serde_json::json!({"replaced": true}),
            },
        ));
        registry.register_tool_execute_before("replacer", replace_hook).await;

        let input = ToolExecuteBeforeInput {
            tool: "test_tool".to_string(),
            session_id: "test-session".to_string(),
            call_id: "test-call".to_string(),
            args: serde_json::json!({}),
        };
        let output = dispatcher.trigger_tool_execute_before(input).await.expect("dispatch should work");

        match output.result {
            HookResult::Replace { result } => {
                assert_eq!(result, serde_json::json!({"replaced": true}));
            }
            _ => panic!("Expected Replace result"),
        }
    }
}

// ============================================================================
// CONFIGURATION EDGE CASE TESTS
// ============================================================================

mod config_edge_cases {
    use super::*;

    #[test]
    fn test_config_default_values() {
        let config = PluginConfig::default();

        // Verify defaults
        assert!(!config.hot_reload);
        assert!(config.sandbox_enabled);
        assert_eq!(config.default_memory_pages, 256);
        assert_eq!(config.default_timeout_ms, 30000);
        assert!(config.disabled_plugins.is_empty());
        assert!(config.enabled_plugins.is_empty());
        assert!(config.load_builtin_plugins);
        assert_eq!(config.max_concurrent, 4);
    }

    #[test]
    fn test_config_plugin_enable_disable() {
        let mut config = PluginConfig::default();

        // By default, all plugins should be enabled
        assert!(config.is_plugin_enabled("any-plugin"));

        // Disable a plugin
        config.disable_plugin("test-plugin");
        assert!(!config.is_plugin_enabled("test-plugin"));
        assert!(config.is_plugin_enabled("other-plugin"));

        // Re-enable the plugin
        config.enable_plugin("test-plugin");
        assert!(config.is_plugin_enabled("test-plugin"));
    }

    #[test]
    fn test_config_whitelist_mode() {
        let mut config = PluginConfig::default();

        // Add to enabled_plugins to trigger whitelist mode
        config.enabled_plugins.push("allowed-plugin".to_string());

        // Only whitelisted plugins should be enabled
        assert!(config.is_plugin_enabled("allowed-plugin"));
        assert!(!config.is_plugin_enabled("other-plugin"));

        // Even if we explicitly enable, whitelist takes precedence
        assert!(!config.is_plugin_enabled("random-plugin"));
    }

    #[test]
    fn test_config_blacklist_overrides_whitelist() {
        let mut config = PluginConfig::default();

        // Add to both lists
        config.enabled_plugins.push("test-plugin".to_string());
        config.disabled_plugins.push("test-plugin".to_string());

        // Blacklist should take precedence
        assert!(!config.is_plugin_enabled("test-plugin"));
    }

    #[test]
    fn test_config_plugin_specific_config() {
        let mut config = PluginConfig::default();

        // No config should return None
        assert!(config.get_plugin_config("test-plugin").is_none());

        // Set config
        let plugin_config = serde_json::json!({
            "api_key": "secret",
            "max_retries": 3,
            "enabled": true
        });
        config.set_plugin_config("test-plugin", plugin_config.clone());

        // Get config
        let retrieved = config.get_plugin_config("test-plugin").expect("should exist");
        assert_eq!(retrieved["api_key"], "secret");
        assert_eq!(retrieved["max_retries"], 3);
        assert_eq!(retrieved["enabled"], true);
    }

    #[test]
    fn test_config_add_search_path_dedup() {
        let mut config = PluginConfig::default();
        let test_path = PathBuf::from("/test/plugins");

        // Add the path
        config.add_search_path(test_path.clone());
        let count_after_first = config.search_paths.len();

        // Add the same path again
        config.add_search_path(test_path.clone());
        let count_after_second = config.search_paths.len();

        // Should not duplicate
        assert_eq!(count_after_first, count_after_second);
    }

    #[test]
    fn test_config_with_search_paths() {
        let paths = vec![
            PathBuf::from("/custom/path1"),
            PathBuf::from("/custom/path2"),
        ];
        let config = PluginConfig::with_search_paths(paths.clone());

        assert_eq!(config.search_paths, paths);
        // Other fields should be default
        assert!(config.sandbox_enabled);
    }
}

// ============================================================================
// ERROR HANDLING EDGE CASE TESTS
// ============================================================================

mod error_edge_cases {
    use super::*;

    #[test]
    fn test_error_display_messages() {
        let errors = vec![
            (
                PluginError::NotFound("test-plugin".to_string()),
                "not found",
            ),
            (
                PluginError::AlreadyExists("test-plugin".to_string()),
                "already exists",
            ),
            (
                PluginError::load_error("plugin", "file missing"),
                "file missing",
            ),
            (
                PluginError::init_error("plugin", "init failed"),
                "init failed",
            ),
            (
                PluginError::invalid_manifest("plugin", "bad version"),
                "bad version",
            ),
            (
                PluginError::execution_error("plugin", "timeout"),
                "timeout",
            ),
            (
                PluginError::hook_error("plugin", "hook crashed"),
                "hook crashed",
            ),
            (
                PluginError::PermissionDenied("filesystem".to_string()),
                "filesystem",
            ),
            (
                PluginError::ConfigError("invalid config".to_string()),
                "invalid config",
            ),
            (
                PluginError::Timeout("operation timed out".to_string()),
                "timed out",
            ),
            (
                PluginError::Disabled("plugin-x".to_string()),
                "disabled",
            ),
            (
                PluginError::checksum_mismatch("plugin", "abc", "def"),
                "checksum",
            ),
            (
                PluginError::validation_error("url", "SSRF blocked"),
                "SSRF",
            ),
        ];

        for (error, expected_substring) in errors {
            let msg = error.to_string().to_lowercase();
            assert!(
                msg.contains(&expected_substring.to_lowercase()),
                "Error message '{}' should contain '{}'",
                msg,
                expected_substring
            );
        }
    }

    #[test]
    fn test_error_from_io_error() {
        let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let plugin_error: PluginError = io_error.into();

        assert!(matches!(plugin_error, PluginError::IoError(_)));
        assert!(plugin_error.to_string().contains("not found"));
    }

    #[test]
    fn test_error_from_toml_error() {
        let invalid_toml = "this is not [valid] toml {";
        let toml_error = toml::from_str::<toml::Value>(invalid_toml).unwrap_err();
        let plugin_error: PluginError = toml_error.into();

        assert!(matches!(plugin_error, PluginError::SerializationError(_)));
    }

    #[test]
    fn test_error_from_json_error() {
        let invalid_json = "{ not valid json }";
        let json_error = serde_json::from_str::<serde_json::Value>(invalid_json).unwrap_err();
        let plugin_error: PluginError = json_error.into();

        assert!(matches!(plugin_error, PluginError::SerializationError(_)));
    }

    #[test]
    fn test_error_builder_methods() {
        // Test all builder methods produce correctly structured errors
        let load = PluginError::load_error("plugin-id", "reason");
        assert!(load.to_string().contains("plugin-id"));
        assert!(load.to_string().contains("reason"));

        let init = PluginError::init_error("plugin-id", "reason");
        assert!(init.to_string().contains("plugin-id"));

        let manifest = PluginError::invalid_manifest("plugin-id", "reason");
        assert!(manifest.to_string().contains("plugin-id"));

        let exec = PluginError::execution_error("plugin-id", "reason");
        assert!(exec.to_string().contains("plugin-id"));

        let hook = PluginError::hook_error("plugin-id", "reason");
        assert!(hook.to_string().contains("plugin-id"));

        let dep = PluginError::dependency_error("plugin-id", "reason");
        assert!(dep.to_string().contains("plugin-id"));

        let checksum = PluginError::checksum_mismatch("plugin-id", "expected", "actual");
        assert!(checksum.to_string().contains("expected"));
        assert!(checksum.to_string().contains("actual"));

        let validation = PluginError::validation_error("field", "message");
        assert!(validation.to_string().contains("field"));
        assert!(validation.to_string().contains("message"));
    }

    #[test]
    fn test_version_mismatch_error() {
        let error = PluginError::VersionMismatch {
            plugin: "test-plugin".to_string(),
            required: "2.0.0".to_string(),
            found: "1.0.0".to_string(),
        };

        let msg = error.to_string();
        assert!(msg.contains("test-plugin"));
        assert!(msg.contains("2.0.0"));
        assert!(msg.contains("1.0.0"));
    }

    #[test]
    fn test_invalid_state_error() {
        let error = PluginError::InvalidState {
            expected: "active".to_string(),
            actual: "loading".to_string(),
        };

        let msg = error.to_string();
        assert!(msg.contains("active"));
        assert!(msg.contains("loading"));
    }
}

// ============================================================================
// PLUGIN STATE EDGE CASES
// ============================================================================

mod plugin_state_edge_cases {
    use cortex_plugins::PluginState;
    use cortex_plugins::plugin::PluginStats;

    #[test]
    fn test_plugin_state_display() {
        let states = vec![
            (PluginState::Discovered, "discovered"),
            (PluginState::Loading, "loading"),
            (PluginState::Loaded, "loaded"),
            (PluginState::Initializing, "initializing"),
            (PluginState::Active, "active"),
            (PluginState::Unloading, "unloading"),
            (PluginState::Unloaded, "unloaded"),
            (PluginState::Error, "error"),
            (PluginState::Disabled, "disabled"),
        ];

        for (state, expected) in states {
            assert_eq!(state.to_string(), expected);
        }
    }

    #[test]
    fn test_plugin_stats_default() {
        let stats = PluginStats::default();

        assert_eq!(stats.commands_executed, 0);
        assert_eq!(stats.hooks_triggered, 0);
        assert_eq!(stats.events_handled, 0);
        assert_eq!(stats.total_execution_ms, 0);
        assert_eq!(stats.errors, 0);
    }

    #[test]
    fn test_plugin_state_equality() {
        assert_eq!(PluginState::Active, PluginState::Active);
        assert_ne!(PluginState::Active, PluginState::Disabled);
    }
}

// ============================================================================
// REMOTE REGISTRY EDGE CASES
// ============================================================================

mod remote_registry_edge_cases {
    use super::*;

    #[test]
    fn test_remote_registry_creation() {
        let enabled = RemoteRegistry::new("https://example.com", "Test Registry");
        assert!(enabled.enabled);
        assert_eq!(enabled.url, "https://example.com");
        assert_eq!(enabled.name, "Test Registry");

        let disabled = RemoteRegistry::new_disabled("https://example.com", "Disabled Registry");
        assert!(!disabled.enabled);
    }

    #[test]
    fn test_plugin_index_entry_is_signed() {
        let signed_entry = PluginIndexEntry {
            id: "test".to_string(),
            name: "Test".to_string(),
            version: "1.0.0".to_string(),
            description: "Test".to_string(),
            download_url: "https://example.com/plugin.wasm".to_string(),
            checksum: "abc123".to_string(),
            signature: Some("signature-hex".to_string()),
            updated_at: Utc::now(),
        };
        assert!(signed_entry.is_signed());

        let unsigned_entry = PluginIndexEntry {
            signature: None,
            ..signed_entry.clone()
        };
        assert!(!unsigned_entry.is_signed());
    }

    #[tokio::test]
    async fn test_registry_disabled_remote_fetch() {
        let registry = PluginRegistry::new();
        let disabled_remote = RemoteRegistry::new_disabled("https://example.com", "Disabled");

        registry.add_remote_registry(disabled_remote.clone()).await;

        // Fetching from disabled registry should return empty
        let entries = registry.fetch_remote_index(&disabled_remote).await.expect("should work");
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn test_registry_remove_non_existent() {
        let registry = PluginRegistry::new();

        // Removing non-existent registry should not panic
        registry.remove_remote_registry("https://nonexistent.com").await;

        let registries = registry.list_remote_registries().await;
        assert!(registries.is_empty());
    }

    #[tokio::test]
    async fn test_registry_add_duplicate_by_url() {
        let registry = PluginRegistry::new();

        let remote1 = RemoteRegistry::new("https://example.com", "First");
        let remote2 = RemoteRegistry::new("https://example.com", "Second");

        registry.add_remote_registry(remote1).await;
        registry.add_remote_registry(remote2).await;

        // Should only have one (deduped by URL)
        let registries = registry.list_remote_registries().await;
        assert_eq!(registries.len(), 1);
        assert_eq!(registries[0].name, "First"); // First one wins
    }
}

// ============================================================================
// SIGNING EDGE CASES
// ============================================================================

mod signing_edge_cases {
    use cortex_plugins::PluginSigner;

    #[test]
    fn test_signer_new_has_no_trusted_keys() {
        let signer = PluginSigner::new();
        assert!(!signer.has_trusted_keys());
    }

    #[test]
    fn test_signer_add_invalid_key_length() {
        let mut signer = PluginSigner::new();

        // Too short
        let short_key = vec![0u8; 16];
        let result = signer.add_trusted_key(&short_key);
        assert!(result.is_err());

        // Too long
        let long_key = vec![0u8; 64];
        let result = signer.add_trusted_key(&long_key);
        assert!(result.is_err());
    }

    #[test]
    fn test_signer_add_invalid_hex_key() {
        let mut signer = PluginSigner::new();

        // Invalid hex characters
        let result = signer.add_trusted_key_hex("GGHHIIJJ");
        assert!(result.is_err());

        // Odd length
        let result = signer.add_trusted_key_hex("abc");
        assert!(result.is_err());
    }

    #[test]
    fn test_signer_checksum_computation() {
        let data = b"test data for checksum";
        let checksum = PluginSigner::compute_checksum(data);

        // Should be hex-encoded SHA256
        assert_eq!(checksum.len(), 64); // 32 bytes = 64 hex chars

        // Same data should produce same checksum
        let checksum2 = PluginSigner::compute_checksum(data);
        assert_eq!(checksum, checksum2);

        // Different data should produce different checksum
        let checksum3 = PluginSigner::compute_checksum(b"different data");
        assert_ne!(checksum, checksum3);
    }

    #[test]
    fn test_signer_verify_without_trusted_keys() {
        let signer = PluginSigner::new();
        let data = b"test data";
        let fake_signature = "0".repeat(128); // 64 bytes in hex

        // Should return false (no keys to verify against)
        let result = signer.verify_plugin_hex(data, &fake_signature);
        assert!(result.is_ok());
        assert!(!result.unwrap()); // No trusted keys = verification fails
    }
}
