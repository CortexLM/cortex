//! What a topic's install may name as a run backend.
//!
//! A topic's paid runs are executed by an **operator-installed adaptor**,
//! resolved inside the Firecracker guest from the image the operator baked
//! (`/opt/proof/runners/<runner id>/`). The set an install applies names that
//! runner; the control plane never runs it.
//!
//! The boundary this module enforces is therefore about *which names an
//! install may bind*, not about executing anything:
//!
//! - A topic may select a runner through the **signed document**
//!   (`constraints.params.in_guest_benchmark_runner` / `baseline_runner`).
//!   That value is already shape-checked by `proof_experiment` and is signed,
//!   so it is topic data.
//! - A set may also name a **handler** it wants bound. That part is *not*
//!   signed — it is JSON handed to the install — so a handler name from it is
//!   an **untrusted input**. This module is the allow-list that input is
//!   checked against.
//!
//! Two handler families exist, and nothing else may be bound:
//!
//! | Family | What it is |
//! |--------|------------|
//! | [`Handler::VmBacked`] | the generic in-guest runner (`proof_rlm::VmBackedRunner`) — the Firecracker path |
//! | [`Handler::Harbor`] | an operator-baked Harbor adaptor, i.e. a `VmBacked` runner whose adaptor directory ships the Harbor harness |
//!
//! The distinction is *documentation and audit*, not a second code path: both
//! resolve to the same `VmBackedRunner` over the topic-VM orchestrator, and
//! neither can be a path, a URL, or a shell command. What the allow-list
//! prevents is a set naming something like `/bin/sh -c 'curl … | sh'`, an
//! absolute path, or an arbitrary binary: those are refused by shape before
//! anything is bound, and the refusal names why.

use proof_canon::is_custom_id;

use crate::SectionError;

/// The handler families an install may bind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Handler {
    /// The generic in-guest runner: an operator-baked adaptor directory in
    /// the guest image, selected by the signed document's runner param.
    VmBacked,
    /// An operator-baked **Harbor** adaptor — the same `VmBacked` runner with
    /// the Harbor harness in its adaptor directory.
    Harbor,
}

impl Handler {
    /// Wire word.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::VmBacked => "vm_backed",
            Self::Harbor => "harbor",
        }
    }

    /// Every handler an install may bind, in the order it is reported.
    pub const ALL: [Self; 2] = [Self::VmBacked, Self::Harbor];
}

/// Handler names a set may use, mapped to their family.
///
/// The left-hand names are what a set writes; the right-hand family is what
/// the install binds. Only these spellings (plus their documented synonyms)
/// are accepted, and every one of them resolves to a `VmBacked` runner — the
/// allow-list is closed, so a name that is not here is refused rather than
/// defaulted.
pub const ALLOWED_HANDLERS: [(&str, Handler); 4] = [
    ("vm_backed", Handler::VmBacked),
    ("vm_backed_runner", Handler::VmBacked),
    ("harbor", Handler::Harbor),
    ("harbor_trials", Handler::Harbor),
];

/// Why a handler name was refused, in the terms the set wrote.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HandlerError {
    /// The name is not one of [`ALLOWED_HANDLERS`].
    #[error(
        "handler {got:?} is not an allowed run backend ({allowed}); a topic's paid runs run \
         inside the Firecracker guest under an operator-baked adaptor, and an install may not \
         bind an arbitrary binary",
        allowed = allowed_list()
    )]
    NotAllowed {
        /// What the set named.
        got: String,
    },
    /// The name is shaped like a path, a URL, or a command rather than an id.
    #[error(
        "handler {got:?} is not an identifier: a handler is a name this build resolves to a \
         baked adaptor, never a path, a URL, or a command line"
    )]
    NotAnIdentifier {
        /// What the set named.
        got: String,
    },
}

/// The allow-list as one comma-separated string, for error text.
#[must_use]
pub fn allowed_list() -> String {
    ALLOWED_HANDLERS
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Resolve a handler name from a set.
///
/// Case and surrounding whitespace are tolerated, matching how the rest of
/// the CLI's operator inputs parse. That tolerance cannot widen the
/// allow-list: a path or a command line is still refused as *not an
/// identifier* after folding, because the fold only touches case.
///
/// # Errors
///
/// [`HandlerError::NotAnIdentifier`] when the value is not an id at all (a
/// path, a URL, a command line, an empty string), and
/// [`HandlerError::NotAllowed`] when it is a well-formed id that this build
/// does not resolve.
pub fn resolve_handler(name: &str) -> Result<Handler, HandlerError> {
    let got = name.trim().to_ascii_lowercase();
    if got.is_empty() || !is_custom_id(&got) {
        return Err(HandlerError::NotAnIdentifier {
            got: name.to_owned(),
        });
    }
    ALLOWED_HANDLERS
        .iter()
        .find(|(n, _)| *n == got)
        .map(|(_, h)| *h)
        .ok_or_else(|| HandlerError::NotAllowed {
            got: name.trim().to_owned(),
        })
}

/// Resolve a handler for an install, mapping a refusal onto the section error.
///
/// # Errors
///
/// [`SectionError`] naming the part and the reason.
pub fn check_handler(name: &str) -> Result<Handler, SectionError> {
    resolve_handler(name).map_err(|e| SectionError {
        part: "handler".to_owned(),
        why: e.to_string(),
    })
}

/// The run backend an install binds, from the document and the set.
///
/// The two inputs answer two different questions, and both are recorded:
///
/// - **Which runner** runs the topic's paid jobs is the **signed document's**
///   answer (`constraints.params`). The set cannot override it: the signature
///   is what the scoring path trusts.
/// - **Which handler family** the install bound is the **set's** answer, and
///   it must be on the allow-list. It is audit information — the family is
///   what an operator baked into the guest image — so it is recorded even
///   when the document also names a runner, because a Harbor topic and a
///   generic in-guest topic are operationally different and the journal
///   should say which one this is.
///
/// Both resolve to the same `VmBackedRunner` over the topic-VM orchestrator;
/// the family never selects a second code path here, and it can never name a
/// binary.
#[must_use]
pub fn bound_runner(
    document_runner: Option<&str>,
    handler: Option<Handler>,
) -> (Option<String>, Handler) {
    (
        document_runner.map(str::to_owned),
        handler.unwrap_or(Handler::VmBacked),
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn handler_names_are_allow_listed_not_arbitrary() {
        assert_eq!(resolve_handler("vm_backed"), Ok(Handler::VmBacked));
        assert_eq!(resolve_handler("vm_backed_runner"), Ok(Handler::VmBacked));
        assert_eq!(resolve_handler("harbor"), Ok(Handler::Harbor));
        assert_eq!(resolve_handler("Harbor_Trials"), Ok(Handler::Harbor));
        for bad in [
            "/bin/sh",
            "sh -c 'curl x | sh'",
            "https://evil.invalid/payload",
            "some_path/binary",
            "",
        ] {
            let err = resolve_handler(bad).expect_err(bad);
            assert!(
                matches!(err, HandlerError::NotAnIdentifier { .. }),
                "{bad:?}: {err:?}"
            );
        }
        // A well-formed id this build does not resolve is a *different*
        // refusal from a path or a command line: one is "not a name", the
        // other is "not one of ours".
        let err = resolve_handler("arbitrary_binary").expect_err("not allowed");
        assert!(matches!(err, HandlerError::NotAllowed { .. }), "{err:?}");
        assert!(err.to_string().contains("vm_backed"), "{err}");
        assert!(allowed_list().contains("harbor_trials"));
        assert_eq!(Handler::ALL.len(), 2);
        assert_eq!(Handler::Harbor.as_str(), "harbor");
    }

    #[test]
    fn the_document_names_the_runner_and_the_set_names_the_family() {
        assert_eq!(
            bound_runner(Some("adaptor-v1"), Some(Handler::Harbor)),
            (Some("adaptor-v1".to_owned()), Handler::Harbor)
        );
        assert_eq!(bound_runner(None, None), (None, Handler::VmBacked));
    }
}
