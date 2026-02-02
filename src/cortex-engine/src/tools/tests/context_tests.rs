//! Tests for tool context.

use crate::tools::context::ToolContext;
use cortex_protocol::SandboxPolicy;
use std::path::PathBuf;

#[test]
fn test_tool_context_new() {
    let cwd = PathBuf::from("/project");
    let ctx = ToolContext::new(cwd.clone());

    assert_eq!(ctx.cwd, cwd);
    assert!(!ctx.auto_approve);
    assert!(ctx.turn_id.is_empty());
    assert!(ctx.conversation_id.is_empty());
}

#[test]
fn test_tool_context_with_sandbox_policy() {
    let cwd = PathBuf::from("/project");
    let ctx = ToolContext::new(cwd).with_sandbox_policy(SandboxPolicy::ReadOnly);

    assert!(matches!(ctx.sandbox_policy, SandboxPolicy::ReadOnly));
}

#[test]
fn test_tool_context_with_turn_id() {
    let cwd = PathBuf::from("/project");
    let ctx = ToolContext::new(cwd).with_turn_id("turn_123");

    assert_eq!(ctx.turn_id, "turn_123");
}

#[test]
fn test_tool_context_with_conversation_id() {
    let cwd = PathBuf::from("/project");
    let ctx = ToolContext::new(cwd).with_conversation_id("conv_456");

    assert_eq!(ctx.conversation_id, "conv_456");
}

#[test]
fn test_tool_context_with_auto_approve() {
    let cwd = PathBuf::from("/project");
    let ctx = ToolContext::new(cwd).with_auto_approve(true);

    assert!(ctx.auto_approve);
}

#[test]
fn test_tool_context_chained_builders() {
    let ctx = ToolContext::new(PathBuf::from("/workspace"))
        .with_sandbox_policy(SandboxPolicy::DangerFullAccess)
        .with_turn_id("t1")
        .with_conversation_id("c1")
        .with_auto_approve(true);

    assert!(matches!(
        ctx.sandbox_policy,
        SandboxPolicy::DangerFullAccess
    ));
    assert_eq!(ctx.turn_id, "t1");
    assert_eq!(ctx.conversation_id, "c1");
    assert!(ctx.auto_approve);
}

#[test]
#[cfg_attr(windows, ignore = "Unix paths not applicable on Windows")]
fn test_tool_context_resolve_path_absolute() {
    let ctx = ToolContext::new(PathBuf::from("/project"));

    let resolved = ctx.resolve_path("/etc/hosts");
    assert_eq!(resolved, PathBuf::from("/etc/hosts"));
}

#[test]
#[cfg_attr(windows, ignore = "Unix paths not applicable on Windows")]
fn test_tool_context_resolve_path_relative() {
    let ctx = ToolContext::new(PathBuf::from("/project"));

    let resolved = ctx.resolve_path("src/main.rs");
    assert_eq!(resolved, PathBuf::from("/project/src/main.rs"));
}

#[test]
#[cfg_attr(windows, ignore = "Unix paths not applicable on Windows")]
fn test_tool_context_resolve_path_dot() {
    let ctx = ToolContext::new(PathBuf::from("/project"));

    let resolved = ctx.resolve_path("./Cargo.toml");
    assert_eq!(resolved, PathBuf::from("/project/./Cargo.toml"));
}

#[test]
#[cfg_attr(windows, ignore = "Unix paths not applicable on Windows")]
fn test_tool_context_resolve_path_parent() {
    let ctx = ToolContext::new(PathBuf::from("/project/src"));

    let resolved = ctx.resolve_path("../README.md");
    // Accept both canonicalized and non-canonicalized paths
    let resolved_str = resolved.to_string_lossy();
    assert!(
        resolved_str.ends_with("README.md"),
        "Path should end with README.md, got: {}",
        resolved_str
    );
}

#[test]
fn test_tool_context_inherits_environment() {
    // Set a test env var
    let test_key = "CORTEX_TEST_CONTEXT_VAR";
    let test_value = "test_value_123";

    // SAFETY: Test environment
    unsafe {
        std::env::set_var(test_key, test_value);
    }

    let ctx = ToolContext::new(PathBuf::from("/tmp"));

    assert!(ctx.env.contains_key(test_key));
    assert_eq!(ctx.env.get(test_key).map(|s| s.as_str()), Some(test_value));

    // Cleanup
    unsafe {
        std::env::remove_var(test_key);
    }
}

#[test]
fn test_tool_context_default_sandbox_policy() {
    let ctx = ToolContext::new(PathBuf::from("/tmp"));

    // Default should be WorkspaceWrite with default values
    assert!(matches!(
        ctx.sandbox_policy,
        SandboxPolicy::WorkspaceWrite { .. }
    ));
}

#[test]
fn test_tool_context_clone() {
    let ctx = ToolContext::new(PathBuf::from("/project"))
        .with_turn_id("turn1")
        .with_auto_approve(true);

    let cloned = ctx.clone();

    assert_eq!(ctx.cwd, cloned.cwd);
    assert_eq!(ctx.turn_id, cloned.turn_id);
    assert_eq!(ctx.auto_approve, cloned.auto_approve);
}

#[test]
#[cfg_attr(windows, ignore = "Unix paths not applicable on Windows")]
fn test_tool_context_debug() {
    let ctx = ToolContext::new(PathBuf::from("/debug"));
    let debug = format!("{:?}", ctx);

    assert!(debug.contains("ToolContext"));
    assert!(debug.contains("/debug"));
}

#[test]
#[cfg_attr(windows, ignore = "Unix paths not applicable on Windows")]
fn test_tool_context_resolve_empty_path() {
    let ctx = ToolContext::new(PathBuf::from("/project"));

    let resolved = ctx.resolve_path("");
    // Empty path is relative, joins with cwd
    assert_eq!(resolved, PathBuf::from("/project/"));
}

#[test]
fn test_tool_context_with_string_ids() {
    let ctx = ToolContext::new(PathBuf::from("/tmp"))
        .with_turn_id(String::from("owned_turn"))
        .with_conversation_id(String::from("owned_conv"));

    assert_eq!(ctx.turn_id, "owned_turn");
    assert_eq!(ctx.conversation_id, "owned_conv");
}
