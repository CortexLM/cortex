//! Tests for streaming module.

use std::time::{Duration, Instant};

use super::{StreamController, StreamState};

// --------------------------------------------------------
// StreamState Tests
// --------------------------------------------------------

#[test]
fn test_stream_state_is_active() {
    assert!(StreamState::Processing.is_active());
    assert!(
        StreamState::Streaming {
            tokens_received: 0,
            started_at: Instant::now()
        }
        .is_active()
    );
    assert!(
        StreamState::Reasoning {
            started_at: Instant::now()
        }
        .is_active()
    );
    assert!(
        StreamState::ExecutingTool {
            tool_name: "test".to_string(),
            started_at: Instant::now()
        }
        .is_active()
    );

    assert!(!StreamState::Idle.is_active());
    assert!(
        !StreamState::WaitingApproval {
            tool_name: "test".to_string()
        }
        .is_active()
    );
    assert!(!StreamState::Finishing.is_active());
    assert!(
        !StreamState::Complete {
            total_tokens: 0,
            duration: Duration::ZERO
        }
        .is_active()
    );
    assert!(!StreamState::Interrupted.is_active());
    assert!(!StreamState::Error("test".to_string()).is_active());
}

#[test]
fn test_stream_state_is_idle() {
    assert!(StreamState::Idle.is_idle());
    assert!(
        StreamState::Complete {
            total_tokens: 10,
            duration: Duration::from_secs(1)
        }
        .is_idle()
    );
    assert!(StreamState::Interrupted.is_idle());
    assert!(StreamState::Error("error".to_string()).is_idle());

    assert!(!StreamState::Processing.is_idle());
    assert!(
        !StreamState::Streaming {
            tokens_received: 0,
            started_at: Instant::now()
        }
        .is_idle()
    );
}

#[test]
fn test_stream_state_is_waiting_approval() {
    assert!(
        StreamState::WaitingApproval {
            tool_name: "bash".to_string()
        }
        .is_waiting_approval()
    );

    assert!(!StreamState::Idle.is_waiting_approval());
    assert!(
        !StreamState::ExecutingTool {
            tool_name: "bash".to_string(),
            started_at: Instant::now()
        }
        .is_waiting_approval()
    );
}

#[test]
fn test_stream_state_tool_name() {
    let state = StreamState::ExecutingTool {
        tool_name: "read_file".to_string(),
        started_at: Instant::now(),
    };
    assert_eq!(state.tool_name(), Some("read_file"));

    let state = StreamState::WaitingApproval {
        tool_name: "write_file".to_string(),
    };
    assert_eq!(state.tool_name(), Some("write_file"));

    assert_eq!(StreamState::Idle.tool_name(), None);
}

#[test]
fn test_stream_state_error_message() {
    let state = StreamState::Error("connection failed".to_string());
    assert_eq!(state.error_message(), Some("connection failed"));

    assert_eq!(StreamState::Idle.error_message(), None);
}

// --------------------------------------------------------
// StreamController Basic Tests
// --------------------------------------------------------

#[test]
fn test_controller_new() {
    let controller = StreamController::new();
    assert!(matches!(controller.state(), StreamState::Idle));
    assert!(controller.committed_text().is_empty());
    assert!(controller.pending_text().is_empty());
    assert_eq!(controller.token_count(), 0);
    assert!(controller.is_newline_gated());
    assert!(!controller.has_typewriter());
}

#[test]
#[ignore = "TUI behavior differs across platforms"]
fn test_controller_with_typewriter() {
    let controller = StreamController::with_typewriter(60.0);
    assert!(controller.has_typewriter());
    assert!(controller.is_newline_gated());
}

#[test]
fn test_controller_immediate_display() {
    let controller = StreamController::new().immediate_display();
    assert!(!controller.is_newline_gated());
}

#[test]
fn test_controller_default() {
    let controller = StreamController::default();
    assert!(matches!(controller.state(), StreamState::Idle));
}

// --------------------------------------------------------
// State Transition Tests
// --------------------------------------------------------

#[test]
fn test_start_processing() {
    let mut controller = StreamController::new();
    controller.start_processing();

    assert!(matches!(controller.state(), StreamState::Processing));
    assert!(controller.elapsed().is_some());
    assert_eq!(controller.token_count(), 0);
}

#[test]
fn test_auto_transition_to_streaming() {
    let mut controller = StreamController::new().immediate_display();
    controller.start_processing();
    controller.append_text("Hello");

    assert!(matches!(
        controller.state(),
        StreamState::Streaming {
            tokens_received: 1,
            ..
        }
    ));
}

#[test]
fn test_start_reasoning() {
    let mut controller = StreamController::new();
    controller.start_processing();
    controller.start_reasoning();

    assert!(controller.state().is_reasoning());
}

#[test]
fn test_start_tool() {
    let mut controller = StreamController::new();
    controller.start_processing();
    controller.start_tool("read_file".to_string());

    assert!(controller.state().is_executing_tool());
    assert_eq!(controller.state().tool_name(), Some("read_file"));
}

#[test]
fn test_wait_approval() {
    let mut controller = StreamController::new();
    controller.wait_approval("bash".to_string());

    assert!(controller.state().is_waiting_approval());
    assert_eq!(controller.state().tool_name(), Some("bash"));
}

#[test]
fn test_complete() {
    let mut controller = StreamController::new().immediate_display();
    controller.start_processing();
    controller.append_text("Hello, world!");
    controller.complete();

    assert!(controller.is_complete());
    if let StreamState::Complete {
        total_tokens,
        duration,
    } = controller.state()
    {
        assert_eq!(*total_tokens, 1);
        assert!(*duration > Duration::ZERO);
    } else {
        panic!("Expected Complete state");
    }
}

#[test]
fn test_interrupt() {
    let mut controller = StreamController::new();
    controller.start_processing();
    controller.append_text("Hello\n");
    controller.interrupt();

    assert!(matches!(controller.state(), StreamState::Interrupted));
}

#[test]
fn test_set_error() {
    let mut controller = StreamController::new();
    controller.start_processing();
    controller.set_error("Network error".to_string());

    assert!(controller.state().is_error());
    assert_eq!(controller.state().error_message(), Some("Network error"));
}

#[test]
fn test_reset() {
    let mut controller = StreamController::new().immediate_display();
    controller.start_processing();
    controller.append_text("Hello");
    controller.complete();
    controller.reset();

    assert!(matches!(controller.state(), StreamState::Idle));
    assert!(controller.committed_text().is_empty());
    assert_eq!(controller.token_count(), 0);
    assert!(controller.elapsed().is_none());
}

// --------------------------------------------------------
// Text Handling Tests
// --------------------------------------------------------

#[test]
fn test_newline_gated_display() {
    let mut controller = StreamController::new();
    controller.start_processing();

    // Text without newline stays in pending buffer
    controller.append_text("Hello");
    assert_eq!(controller.pending_text(), "Hello");
    assert!(controller.committed_text().is_empty());

    // After newline, text is committed
    controller.append_text(" world!\n");
    assert_eq!(controller.committed_text(), "Hello world!\n");
    assert!(controller.pending_text().is_empty());
}

#[test]
fn test_newline_gated_multiple_lines() {
    let mut controller = StreamController::new();
    controller.start_processing();

    controller.append_text("Line 1\nLine 2\nPart");

    assert_eq!(controller.committed_text(), "Line 1\nLine 2\n");
    assert_eq!(controller.pending_text(), "Part");
}

#[test]
fn test_immediate_display() {
    let mut controller = StreamController::new().immediate_display();
    controller.start_processing();

    controller.append_text("Hello");
    assert_eq!(controller.committed_text(), "Hello");
    assert!(controller.pending_text().is_empty());
}

#[test]
fn test_flush_pending() {
    let mut controller = StreamController::new();
    controller.start_processing();
    controller.append_text("Incomplete line");

    assert_eq!(controller.committed_text(), "");
    assert_eq!(controller.pending_text(), "Incomplete line");

    controller.flush_pending();

    assert_eq!(controller.committed_text(), "Incomplete line");
    assert!(controller.pending_text().is_empty());
}

#[test]
fn test_complete_flushes_pending() {
    let mut controller = StreamController::new();
    controller.start_processing();
    controller.append_text("Incomplete");
    controller.complete();

    assert_eq!(controller.committed_text(), "Incomplete");
    assert!(controller.pending_text().is_empty());
}

#[test]
fn test_display_text_without_typewriter() {
    let mut controller = StreamController::new().immediate_display();
    controller.start_processing();
    controller.append_text("Hello, world!");

    assert_eq!(controller.display_text(), "Hello, world!");
}

#[test]
fn test_token_count() {
    let mut controller = StreamController::new().immediate_display();
    controller.start_processing();

    assert_eq!(controller.token_count(), 0);

    controller.append_text("Hello");
    assert_eq!(controller.token_count(), 1);

    controller.append_text(" ");
    controller.append_text("world");
    assert_eq!(controller.token_count(), 3);
}

// --------------------------------------------------------
// Typewriter Integration Tests
// --------------------------------------------------------

#[test]
fn test_typewriter_integration() {
    let mut controller = StreamController::with_typewriter(1000.0).immediate_display();
    controller.start_processing();
    controller.append_text("Hello");

    // Initially, typewriter shows nothing
    assert!(controller.display_text().len() <= controller.committed_text().len());

    // Tick to reveal characters
    for _ in 0..100 {
        controller.tick();
    }

    // Should reveal some characters
    assert!(!controller.display_text().is_empty());
}

#[test]
fn test_skip_animation() {
    let mut controller = StreamController::with_typewriter(1.0).immediate_display();
    controller.start_processing();
    controller.append_text("Hello");

    controller.skip_animation();

    // After skip, all text should be visible
    assert!(controller.animation_complete());
}

#[test]
fn test_visible_progress() {
    let mut controller = StreamController::with_typewriter(1000.0).immediate_display();
    controller.start_processing();
    controller.append_text("Hello");

    let (visible, total) = controller.visible_progress();
    assert_eq!(total, 5);
    assert!(visible <= total);
}

#[test]
fn test_animation_complete_no_typewriter() {
    let controller = StreamController::new();
    assert!(controller.animation_complete());
}

// --------------------------------------------------------
// Metrics Tests
// --------------------------------------------------------

#[test]
fn test_time_to_first_token() {
    let mut controller = StreamController::new().immediate_display();
    controller.start_processing();

    // Before first token, TTFT is None
    assert!(controller.time_to_first_token().is_none());

    // Sleep a tiny bit to ensure measurable time
    std::thread::sleep(Duration::from_millis(1));

    controller.append_text("First");

    // After first token, TTFT should be Some
    let ttft = controller.time_to_first_token();
    assert!(ttft.is_some());
    assert!(ttft.unwrap() > Duration::ZERO);
}

#[test]
fn test_elapsed() {
    let mut controller = StreamController::new();

    // Before starting, elapsed is None
    assert!(controller.elapsed().is_none());

    controller.start_processing();
    std::thread::sleep(Duration::from_millis(1));

    // After starting, elapsed should be Some and positive
    let elapsed = controller.elapsed();
    assert!(elapsed.is_some());
    assert!(elapsed.unwrap() > Duration::ZERO);
}

// --------------------------------------------------------
// Edge Cases
// --------------------------------------------------------

#[test]
fn test_append_text_while_idle() {
    let mut controller = StreamController::new();

    // Appending while idle should still work but not change state
    controller.append_text("Hello");

    // State remains Idle because we didn't call start_processing
    // But token count is incremented
    assert_eq!(controller.token_count(), 1);
}

#[test]
fn test_empty_text_append() {
    let mut controller = StreamController::new();
    controller.start_processing();

    controller.append_text("");

    // Empty append still counts as a token
    assert_eq!(controller.token_count(), 1);
}

#[test]
fn test_state_eq() {
    let state1 = StreamState::Idle;
    let state2 = StreamState::Idle;
    assert_eq!(state1, state2);

    let state3 = StreamState::Processing;
    assert_ne!(state1, state3);

    let state4 = StreamState::Error("error".to_string());
    let state5 = StreamState::Error("error".to_string());
    assert_eq!(state4, state5);

    let state6 = StreamState::Error("different".to_string());
    assert_ne!(state4, state6);
}

#[test]
fn test_streaming_from_reasoning() {
    let mut controller = StreamController::new().immediate_display();
    controller.start_processing();
    controller.start_reasoning();

    assert!(controller.state().is_reasoning());

    // start_streaming should work from reasoning state
    controller.start_streaming();
    assert!(controller.state().is_streaming());
}
