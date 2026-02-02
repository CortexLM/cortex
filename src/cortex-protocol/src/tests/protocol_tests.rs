//! Comprehensive tests for protocol types.

use crate::ConversationId;
use crate::protocol::*;
use std::path::PathBuf;

#[test]
fn test_submission_serialization() {
    let submission = Submission {
        id: "sub_123".to_string(),
        op: Op::Interrupt,
    };

    let json = serde_json::to_string(&submission).expect("serialize");
    let parsed: Submission = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(parsed.id, "sub_123");
    assert!(matches!(parsed.op, Op::Interrupt));
}

#[test]
fn test_op_user_input_serialization() {
    let op = Op::UserInput {
        items: vec![crate::UserInput::text("Hello")],
    };

    let json = serde_json::to_string(&op).expect("serialize");
    assert!(json.contains("user_input"));

    let parsed: Op = serde_json::from_str(&json).expect("deserialize");
    match parsed {
        Op::UserInput { items } => assert_eq!(items.len(), 1),
        _ => panic!("Expected UserInput"),
    }
}

#[test]
fn test_op_user_turn_full_serialization() {
    use crate::config_types::{ReasoningEffort, ReasoningSummary};

    let op = Op::UserTurn {
        items: vec![crate::UserInput::text("Build me an app")],
        cwd: PathBuf::from("/project"),
        approval_policy: AskForApproval::OnRequest,
        sandbox_policy: SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![PathBuf::from("/tmp")],
            network_access: true,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        },
        model: "gpt-4o".to_string(),
        effort: Some(ReasoningEffort::High),
        summary: ReasoningSummary::Brief,
        final_output_json_schema: None,
    };

    let json = serde_json::to_string(&op).expect("serialize");
    assert!(json.contains("user_turn"));
    assert!(json.contains("gpt-4o"));

    let parsed: Op = serde_json::from_str(&json).expect("deserialize");
    match parsed {
        Op::UserTurn { items, model, .. } => {
            assert_eq!(items.len(), 1);
            assert_eq!(model, "gpt-4o");
        }
        _ => panic!("Expected UserTurn"),
    }
}

#[test]
fn test_op_exec_approval() {
    let op = Op::ExecApproval {
        id: "approval_1".to_string(),
        decision: ReviewDecision::Approved,
    };

    let json = serde_json::to_string(&op).expect("serialize");
    assert!(json.contains("exec_approval"));
    assert!(json.contains("approved"));
}

#[test]
fn test_op_patch_approval() {
    let op = Op::PatchApproval {
        id: "patch_1".to_string(),
        decision: ReviewDecision::ApprovedForSession,
    };

    let json = serde_json::to_string(&op).expect("serialize");
    assert!(json.contains("patch_approval"));
    assert!(json.contains("approved_for_session"));
}

#[test]
fn test_op_resolve_elicitation() {
    let op = Op::ResolveElicitation {
        server_name: "mcp-server".to_string(),
        request_id: "req_123".to_string(),
        decision: ElicitationAction::Approve,
    };

    let json = serde_json::to_string(&op).expect("serialize");
    let parsed: Op = serde_json::from_str(&json).expect("deserialize");

    match parsed {
        Op::ResolveElicitation { server_name, .. } => {
            assert_eq!(server_name, "mcp-server");
        }
        _ => panic!("Expected ResolveElicitation"),
    }
}

#[test]
fn test_op_add_to_history() {
    let op = Op::AddToHistory {
        text: "Previous command".to_string(),
    };

    let json = serde_json::to_string(&op).expect("serialize");
    assert!(json.contains("add_to_history"));
    assert!(json.contains("Previous command"));
}

#[test]
fn test_op_review() {
    let op = Op::Review {
        review_request: ReviewRequest {
            prompt: "Review this code".to_string(),
            user_facing_hint: "Code review in progress".to_string(),
            append_to_original_thread: false,
        },
    };

    let json = serde_json::to_string(&op).expect("serialize");
    assert!(json.contains("review"));
}

#[test]
fn test_event_serialization() {
    let event = Event {
        id: "evt_123".to_string(),
        msg: EventMsg::TaskComplete(TaskCompleteEvent {
            last_agent_message: Some("Task completed successfully!".to_string()),
        }),
    };

    let json = serde_json::to_string(&event).expect("serialize");
    assert!(json.contains("task_complete"));

    let parsed: Event = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.id, "evt_123");
}

#[test]
fn test_session_configured_event() {
    let event = EventMsg::SessionConfigured(Box::new(SessionConfiguredEvent {
        session_id: ConversationId::new(),
        parent_session_id: None,
        model: "claude-3-5-sonnet".to_string(),
        model_provider_id: "anthropic".to_string(),
        approval_policy: AskForApproval::OnRequest,
        sandbox_policy: SandboxPolicy::default(),
        cwd: PathBuf::from("/workspace"),
        reasoning_effort: None,
        history_log_id: 1,
        history_entry_count: 0,
        initial_messages: None,
        rollout_path: PathBuf::from("/rollout/test.json"),
    }));

    let json = serde_json::to_string(&event).expect("serialize");
    assert!(json.contains("session_configured"));
    assert!(json.contains("claude-3-5-sonnet"));
}

#[test]
fn test_task_started_event() {
    let event = EventMsg::TaskStarted(TaskStartedEvent {
        model_context_window: Some(128000),
    });

    let json = serde_json::to_string(&event).expect("serialize");
    assert!(json.contains("task_started"));
    assert!(json.contains("128000"));
}

#[test]
fn test_agent_message_events() {
    let full = EventMsg::AgentMessage(AgentMessageEvent {
        id: Some("msg_1".to_string()),
        parent_id: None,
        message: "Hello, I'm your assistant!".to_string(),
        finish_reason: None,
    });

    let delta = EventMsg::AgentMessageDelta(AgentMessageDeltaEvent {
        delta: "Hello".to_string(),
    });

    let full_json = serde_json::to_string(&full).expect("serialize");
    let delta_json = serde_json::to_string(&delta).expect("serialize");

    assert!(full_json.contains("agent_message"));
    assert!(delta_json.contains("agent_message_delta"));
}

#[test]
fn test_exec_command_events() {
    let begin = EventMsg::ExecCommandBegin(ExecCommandBeginEvent {
        call_id: "call_1".to_string(),
        turn_id: "turn_1".to_string(),
        command: vec!["ls".to_string(), "-la".to_string()],
        cwd: PathBuf::from("/project"),
        parsed_cmd: vec![ParsedCommand {
            program: "ls".to_string(),
            args: vec!["-la".to_string()],
        }],
        source: ExecCommandSource::Agent,
        interaction_input: None,
        tool_name: Some("Execute".to_string()),
        tool_arguments: Some(serde_json::json!({"command": "ls -la"})),
    });

    let json = serde_json::to_string(&begin).expect("serialize");
    assert!(json.contains("exec_command_begin"));
    assert!(json.contains("ls"));
}

#[test]
fn test_exec_command_output_delta() {
    let delta = EventMsg::ExecCommandOutputDelta(ExecCommandOutputDeltaEvent {
        call_id: "call_1".to_string(),
        stream: ExecOutputStream::Stdout,
        chunk: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, "file.txt\n"),
    });

    let json = serde_json::to_string(&delta).expect("serialize");
    assert!(json.contains("exec_command_output_delta"));
    assert!(json.contains("stdout"));
}

#[test]
fn test_exec_command_end_event() {
    let end = EventMsg::ExecCommandEnd(Box::new(ExecCommandEndEvent {
        call_id: "call_1".to_string(),
        turn_id: "turn_1".to_string(),
        command: vec!["echo".to_string(), "hello".to_string()],
        cwd: PathBuf::from("/project"),
        parsed_cmd: vec![ParsedCommand {
            program: "echo".to_string(),
            args: vec!["hello".to_string()],
        }],
        source: ExecCommandSource::Agent,
        interaction_input: None,
        stdout: "hello\n".to_string(),
        stderr: "".to_string(),
        aggregated_output: "hello\n".to_string(),
        exit_code: 0,
        duration_ms: 50,
        formatted_output: "hello\n".to_string(),
        metadata: None,
    }));

    let json = serde_json::to_string(&end).expect("serialize");
    assert!(json.contains("exec_command_end"));
    assert!(json.contains("exit_code"));
}

#[test]
fn test_error_event() {
    let error = EventMsg::Error(ErrorEvent {
        message: "Something went wrong".to_string(),
        cortex_error_info: Some(CortexErrorInfo::ContextWindowExceeded),
    });

    let json = serde_json::to_string(&error).expect("serialize");
    assert!(json.contains("error"));
    assert!(json.contains("context_window_exceeded"));
}

#[test]
fn test_all_cortex_error_info_variants() {
    let variants = vec![
        CortexErrorInfo::ContextWindowExceeded,
        CortexErrorInfo::UsageLimitExceeded,
        CortexErrorInfo::HttpConnectionFailed {
            http_status_code: Some(503),
        },
        CortexErrorInfo::ResponseStreamConnectionFailed {
            http_status_code: Some(502),
        },
        CortexErrorInfo::InternalServerError,
        CortexErrorInfo::Unauthorized,
        CortexErrorInfo::BadRequest,
        CortexErrorInfo::SandboxError,
        CortexErrorInfo::ResponseStreamDisconnected {
            http_status_code: None,
        },
        CortexErrorInfo::ResponseTooManyFailedAttempts {
            http_status_code: Some(429),
        },
        CortexErrorInfo::Other,
    ];

    for variant in variants {
        let json = serde_json::to_string(&variant).expect("serialize");
        let parsed: CortexErrorInfo = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(variant, parsed);
    }
}

#[test]
fn test_ask_for_approval_all_variants() {
    let variants = vec![
        AskForApproval::UnlessTrusted,
        AskForApproval::OnFailure,
        AskForApproval::OnRequest,
        AskForApproval::Never,
    ];

    for variant in variants {
        let json = serde_json::to_string(&variant).expect("serialize");
        let parsed: AskForApproval = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(variant, parsed);
    }
}

#[test]
fn test_sandbox_policy_full_access() {
    let policy = SandboxPolicy::DangerFullAccess;
    let json = serde_json::to_string(&policy).expect("serialize");
    assert!(json.contains("danger-full-access"));

    assert!(policy.has_full_disk_write_access());
    assert!(policy.has_full_network_access());
}

#[test]
fn test_sandbox_policy_read_only() {
    let policy = SandboxPolicy::ReadOnly;
    let json = serde_json::to_string(&policy).expect("serialize");
    assert!(json.contains("read-only"));

    assert!(!policy.has_full_disk_write_access());
    assert!(!policy.has_full_network_access());
}

#[test]
fn test_sandbox_policy_workspace_write() {
    let policy = SandboxPolicy::WorkspaceWrite {
        writable_roots: vec![PathBuf::from("/project"), PathBuf::from("/tmp")],
        network_access: true,
        exclude_tmpdir_env_var: true,
        exclude_slash_tmp: false,
    };

    let json = serde_json::to_string(&policy).expect("serialize");
    assert!(json.contains("workspace-write"));
    assert!(json.contains("/project"));

    assert!(!policy.has_full_disk_write_access());
    assert!(policy.has_full_network_access());
}

#[test]
fn test_sandbox_policy_get_writable_roots() {
    let policy = SandboxPolicy::WorkspaceWrite {
        writable_roots: vec![PathBuf::from("/extra")],
        network_access: false,
        exclude_tmpdir_env_var: true,
        exclude_slash_tmp: true,
    };

    let cwd = PathBuf::from("/workspace");
    let roots = policy.get_writable_roots_with_cwd(&cwd);

    // Should include cwd and /extra at minimum
    assert!(roots.iter().any(|r| r.root == cwd));
    assert!(
        roots
            .iter()
            .any(|r| r.root == std::path::Path::new("/extra"))
    );
}

#[test]
fn test_writable_root_path_check() {
    let root = WritableRoot {
        root: PathBuf::from("/project"),
        read_only_subpaths: vec![PathBuf::from("/project/.git")],
    };

    assert!(root.is_path_writable(&PathBuf::from("/project/src/main.rs")));
    assert!(root.is_path_writable(&PathBuf::from("/project/Cargo.toml")));
    assert!(!root.is_path_writable(&PathBuf::from("/project/.git/HEAD")));
    assert!(!root.is_path_writable(&PathBuf::from("/other/file.txt")));
}

#[test]
fn test_review_decision_all_variants() {
    let variants = vec![
        ReviewDecision::Approved,
        ReviewDecision::ApprovedForSession,
        ReviewDecision::Denied,
        ReviewDecision::Abort,
    ];

    for variant in variants {
        let json = serde_json::to_string(&variant).expect("serialize");
        let parsed: ReviewDecision = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(variant, parsed);
    }
}

#[test]
fn test_mcp_events() {
    let begin = EventMsg::McpToolCallBegin(McpToolCallBeginEvent {
        call_id: "mcp_1".to_string(),
        invocation: McpInvocation {
            server: "github-server".to_string(),
            tool: "search_repositories".to_string(),
            arguments: Some(serde_json::json!({"query": "rust"})),
        },
    });

    let end = EventMsg::McpToolCallEnd(McpToolCallEndEvent {
        call_id: "mcp_1".to_string(),
        invocation: McpInvocation {
            server: "github-server".to_string(),
            tool: "search_repositories".to_string(),
            arguments: Some(serde_json::json!({"query": "rust"})),
        },
        duration_ms: 500,
        result: Ok(McpToolResult {
            content: vec![McpContent::Text {
                text: "Found 100 repos".to_string(),
            }],
            is_error: Some(false),
        }),
    });

    let begin_json = serde_json::to_string(&begin).expect("serialize");
    let end_json = serde_json::to_string(&end).expect("serialize");

    assert!(begin_json.contains("mcp_tool_call_begin"));
    assert!(end_json.contains("mcp_tool_call_end"));
}

#[test]
fn test_mcp_startup_events() {
    let update = EventMsg::McpStartupUpdate(McpStartupUpdateEvent {
        server: "test-server".to_string(),
        status: McpStartupStatus::Starting,
    });

    let complete = EventMsg::McpStartupComplete(McpStartupCompleteEvent {
        ready: vec!["server1".to_string(), "server2".to_string()],
        failed: vec![McpStartupFailure {
            server: "server3".to_string(),
            error: "Connection refused".to_string(),
        }],
        cancelled: vec!["server4".to_string()],
    });

    let update_json = serde_json::to_string(&update).expect("serialize");
    let complete_json = serde_json::to_string(&complete).expect("serialize");

    assert!(update_json.contains("mcp_startup_update"));
    assert!(complete_json.contains("mcp_startup_complete"));
}

#[test]
fn test_patch_events() {
    use std::collections::HashMap;

    let mut changes = HashMap::new();
    changes.insert(
        PathBuf::from("/project/src/main.rs"),
        FileChange::Update {
            unified_diff: "@@ -1,3 +1,4 @@\n+// New line\n fn main() {}".to_string(),
            move_path: None,
        },
    );
    changes.insert(
        PathBuf::from("/project/src/new.rs"),
        FileChange::Add {
            content: "// New file\n".to_string(),
        },
    );

    let begin = EventMsg::PatchApplyBegin(PatchApplyBeginEvent {
        call_id: "patch_1".to_string(),
        turn_id: "turn_1".to_string(),
        auto_approved: true,
        changes: changes.clone(),
    });

    let json = serde_json::to_string(&begin).expect("serialize");
    assert!(json.contains("patch_apply_begin"));
}

#[test]
fn test_token_count_event() {
    let event = EventMsg::TokenCount(TokenCountEvent {
        info: Some(TokenUsageInfo {
            total_token_usage: TokenUsage {
                input_tokens: 1000,
                cached_input_tokens: 200,
                output_tokens: 500,
                reasoning_output_tokens: 100,
                total_tokens: 1800,
            },
            last_token_usage: TokenUsage::default(),
            model_context_window: Some(128000),
            context_tokens: 1500,
        }),
        rate_limits: Some(RateLimitSnapshot {
            primary: Some(RateLimitWindow {
                used_percent: 25.5,
                window_minutes: Some(60),
                resets_at: Some(1700000000),
            }),
            secondary: None,
            credits: Some(CreditsSnapshot {
                has_credits: true,
                unlimited: false,
                balance: Some("$10.00".to_string()),
            }),
        }),
    });

    let json = serde_json::to_string(&event).expect("serialize");
    assert!(json.contains("token_count"));
    assert!(json.contains("1000"));
}

#[test]
fn test_undo_events() {
    let started = EventMsg::UndoStarted(UndoStartedEvent {
        message: Some("Undoing last operation...".to_string()),
    });

    let completed = EventMsg::UndoCompleted(UndoCompletedEvent {
        success: true,
        message: Some("Undo successful".to_string()),
    });

    let started_json = serde_json::to_string(&started).expect("serialize");
    let completed_json = serde_json::to_string(&completed).expect("serialize");

    assert!(started_json.contains("undo_started"));
    assert!(completed_json.contains("undo_completed"));
}

#[test]
fn test_review_events() {
    let review_request = ReviewRequest {
        prompt: "Review for security issues".to_string(),
        user_facing_hint: "Security review".to_string(),
        append_to_original_thread: false,
    };

    let entered = EventMsg::EnteredReviewMode(review_request.clone());

    let exited = EventMsg::ExitedReviewMode(ExitedReviewModeEvent {
        review_output: Some(ReviewOutputEvent {
            findings: vec![ReviewFinding {
                title: "SQL Injection Vulnerability".to_string(),
                body: "User input not sanitized".to_string(),
                confidence_score: 0.95,
                priority: 1,
                code_location: ReviewCodeLocation {
                    absolute_file_path: PathBuf::from("/project/src/db.rs"),
                    line_range: ReviewLineRange { start: 42, end: 45 },
                },
            }],
            overall_correctness: "Needs attention".to_string(),
            overall_explanation: "Found security issues".to_string(),
            overall_confidence_score: 0.9,
        }),
    });

    let entered_json = serde_json::to_string(&entered).expect("serialize");
    let exited_json = serde_json::to_string(&exited).expect("serialize");

    assert!(entered_json.contains("entered_review_mode"));
    assert!(exited_json.contains("exited_review_mode"));
}

#[test]
fn test_plan_update_event() {
    let event = EventMsg::PlanUpdate(PlanUpdateEvent {
        plan: vec![
            PlanItem {
                id: "1".to_string(),
                title: "Setup project".to_string(),
                status: PlanItemStatus::Completed,
            },
            PlanItem {
                id: "2".to_string(),
                title: "Implement feature".to_string(),
                status: PlanItemStatus::InProgress,
            },
            PlanItem {
                id: "3".to_string(),
                title: "Write tests".to_string(),
                status: PlanItemStatus::Pending,
            },
        ],
    });

    let json = serde_json::to_string(&event).expect("serialize");
    assert!(json.contains("plan_update"));
    assert!(json.contains("completed"));
    assert!(json.contains("in_progress"));
    assert!(json.contains("pending"));
}

#[test]
fn test_web_search_events() {
    let begin = EventMsg::WebSearchBegin(WebSearchBeginEvent {
        call_id: "search_1".to_string(),
    });

    let end = EventMsg::WebSearchEnd(WebSearchEndEvent {
        call_id: "search_1".to_string(),
        query: "rust programming language".to_string(),
    });

    let begin_json = serde_json::to_string(&begin).expect("serialize");
    let end_json = serde_json::to_string(&end).expect("serialize");

    assert!(begin_json.contains("web_search_begin"));
    assert!(end_json.contains("web_search_end"));
    assert!(end_json.contains("rust programming language"));
}

#[test]
fn test_turn_abort_reasons() {
    let reasons = vec![
        TurnAbortReason::Interrupted,
        TurnAbortReason::Replaced,
        TurnAbortReason::ReviewEnded,
    ];

    for reason in reasons {
        let event = EventMsg::TurnAborted(TurnAbortedEvent {
            reason: reason.clone(),
        });
        let json = serde_json::to_string(&event).expect("serialize");
        let parsed: EventMsg = serde_json::from_str(&json).expect("deserialize");

        if let EventMsg::TurnAborted(e) = parsed {
            assert_eq!(e.reason, reason);
        } else {
            panic!("Expected TurnAborted");
        }
    }
}

#[test]
fn test_exec_command_source_all_variants() {
    let sources = vec![
        ExecCommandSource::Agent,
        ExecCommandSource::UserShell,
        ExecCommandSource::UnifiedExecStartup,
        ExecCommandSource::UnifiedExecInteraction,
    ];

    for source in sources {
        let json = serde_json::to_string(&source).expect("serialize");
        let parsed: ExecCommandSource = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(source, parsed);
    }
}

#[test]
fn test_deprecation_notice() {
    let event = EventMsg::DeprecationNotice(DeprecationNoticeEvent {
        summary: "API v1 deprecated".to_string(),
        details: Some("Please migrate to v2 by 2025".to_string()),
    });

    let json = serde_json::to_string(&event).expect("serialize");
    assert!(json.contains("deprecation_notice"));
    assert!(json.contains("API v1 deprecated"));
}

#[test]
fn test_fork_session_op() {
    let op = Op::ForkSession {
        fork_point_message_id: Some("msg_123".to_string()),
        message_index: None,
    };

    let json = serde_json::to_string(&op).expect("serialize");
    assert!(json.contains("fork_session"));
    assert!(json.contains("msg_123"));

    let parsed: Op = serde_json::from_str(&json).expect("deserialize");
    if let Op::ForkSession {
        fork_point_message_id,
        ..
    } = parsed
    {
        assert_eq!(fork_point_message_id, Some("msg_123".to_string()));
    } else {
        panic!("Expected ForkSession");
    }
}

#[test]
fn test_get_session_timeline_op() {
    let op = Op::GetSessionTimeline;

    let json = serde_json::to_string(&op).expect("serialize");
    assert!(json.contains("get_session_timeline"));

    let parsed: Op = serde_json::from_str(&json).expect("deserialize");
    assert!(matches!(parsed, Op::GetSessionTimeline));
}

#[test]
fn test_session_forked_event() {
    let parent_id = ConversationId::new();
    let new_id = ConversationId::new();
    let event = EventMsg::SessionForked(SessionForkedEvent {
        new_session_id: new_id,
        parent_session_id: parent_id,
        fork_point_message_id: Some("msg_456".to_string()),
    });

    let json = serde_json::to_string(&event).expect("serialize");
    assert!(json.contains("session_forked"));
    assert!(json.contains("msg_456"));

    let parsed: EventMsg = serde_json::from_str(&json).expect("deserialize");
    if let EventMsg::SessionForked(e) = parsed {
        assert_eq!(e.new_session_id, new_id);
        assert_eq!(e.parent_session_id, parent_id);
    } else {
        panic!("Expected SessionForked");
    }
}

#[test]
fn test_timeline_updated_event() {
    let session_id = ConversationId::new();
    let parent_id = ConversationId::new();
    let child1 = ConversationId::new();
    let child2 = ConversationId::new();

    let event = EventMsg::TimelineUpdated(TimelineUpdatedEvent {
        session_id,
        parent_session_id: Some(parent_id),
        child_session_ids: vec![child1, child2],
    });

    let json = serde_json::to_string(&event).expect("serialize");
    assert!(json.contains("timeline_updated"));

    let parsed: EventMsg = serde_json::from_str(&json).expect("deserialize");
    if let EventMsg::TimelineUpdated(e) = parsed {
        assert_eq!(e.session_id, session_id);
        assert_eq!(e.parent_session_id, Some(parent_id));
        assert_eq!(e.child_session_ids.len(), 2);
    } else {
        panic!("Expected TimelineUpdated");
    }
}

#[test]
fn test_unknown_event_type_deserialization() {
    // Test that unknown event types deserialize to the Unknown variant
    // instead of failing. This ensures forward compatibility.
    let json = r#"{"type":"future_event_type_not_yet_implemented","data":"some data"}"#;
    let parsed: EventMsg = serde_json::from_str(json).expect("deserialize unknown event");
    assert!(matches!(parsed, EventMsg::Unknown));
}

#[test]
fn test_part_timing_duration_checked_arithmetic() {
    use crate::protocol::PartTiming;

    // Normal case: end > start
    let timing = PartTiming {
        start: 1000,
        end: Some(2000),
        compacted: None,
    };
    assert_eq!(timing.duration_ms(), Some(1000));

    // Edge case: end == start
    let timing = PartTiming {
        start: 1000,
        end: Some(1000),
        compacted: None,
    };
    assert_eq!(timing.duration_ms(), Some(0));

    // Edge case: end is None
    let timing = PartTiming {
        start: 1000,
        end: None,
        compacted: None,
    };
    assert_eq!(timing.duration_ms(), None);

    // Corrupted data: end < start should return None (not panic or overflow)
    let timing = PartTiming {
        start: 2000,
        end: Some(1000),
        compacted: None,
    };
    assert_eq!(timing.duration_ms(), None);

    // Extreme case: very large timestamps
    let timing = PartTiming {
        start: i64::MAX - 1000,
        end: Some(i64::MAX),
        compacted: None,
    };
    assert_eq!(timing.duration_ms(), Some(1000));
}

#[test]
fn test_boxed_event_variants() {
    // Test that boxed variants work correctly with serialization/deserialization

    // SessionConfigured (boxed)
    let event = EventMsg::SessionConfigured(Box::new(SessionConfiguredEvent {
        session_id: ConversationId::new(),
        parent_session_id: None,
        model: "test-model".to_string(),
        model_provider_id: "test-provider".to_string(),
        approval_policy: AskForApproval::OnRequest,
        sandbox_policy: SandboxPolicy::default(),
        cwd: PathBuf::from("/test"),
        reasoning_effort: None,
        history_log_id: 1,
        history_entry_count: 0,
        initial_messages: None,
        rollout_path: PathBuf::from("/rollout"),
    }));
    let json = serde_json::to_string(&event).expect("serialize boxed SessionConfigured");
    let parsed: EventMsg =
        serde_json::from_str(&json).expect("deserialize boxed SessionConfigured");
    assert!(matches!(parsed, EventMsg::SessionConfigured(_)));

    // ExecCommandEnd (boxed)
    let event = EventMsg::ExecCommandEnd(Box::new(ExecCommandEndEvent {
        call_id: "call_1".to_string(),
        turn_id: "turn_1".to_string(),
        command: vec!["test".to_string()],
        cwd: PathBuf::from("/test"),
        parsed_cmd: vec![],
        source: ExecCommandSource::Agent,
        interaction_input: None,
        stdout: "".to_string(),
        stderr: "".to_string(),
        aggregated_output: "".to_string(),
        exit_code: 0,
        duration_ms: 0,
        formatted_output: "".to_string(),
        metadata: None,
    }));
    let json = serde_json::to_string(&event).expect("serialize boxed ExecCommandEnd");
    let parsed: EventMsg = serde_json::from_str(&json).expect("deserialize boxed ExecCommandEnd");
    assert!(matches!(parsed, EventMsg::ExecCommandEnd(_)));

    // McpListToolsResponse (boxed)
    let event = EventMsg::McpListToolsResponse(Box::new(McpListToolsResponseEvent {
        tools: std::collections::HashMap::new(),
        resources: std::collections::HashMap::new(),
        resource_templates: std::collections::HashMap::new(),
        auth_statuses: std::collections::HashMap::new(),
    }));
    let json = serde_json::to_string(&event).expect("serialize boxed McpListToolsResponse");
    let parsed: EventMsg =
        serde_json::from_str(&json).expect("deserialize boxed McpListToolsResponse");
    assert!(matches!(parsed, EventMsg::McpListToolsResponse(_)));
}
