//! What a topic's RLM install may name as a run backend.
//!
//! A topic's paid runs are executed by an **operator-installed adaptor**,
//! resolved inside the Firecracker guest from the image the operator baked
//! (`/opt/proof/runners/<runner id>/`). The bundle's RLM section names that
//! runner; the control plane never runs it.
//!
//! The boundary this module enforces is therefore about *which names an
//! install may bind*, not about executing anything:
//!
//! - A topic may select a runner through the **signed document**
//!   (`constraints.params.in_guest_benchmark_runner` / `baseline_runner`).
//!   That value is already shape-checked by [`proof_experiment`] and is
//!   signed, so it is topic data.
//! - A bundle's `rlm` section may also name a **handler** it wants bound.
//!   That section is *not* signed — it is operator-supplied JSON handed to
//!   the install — so a handler name from it is an **untrusted input**. This
//!   module is the allow-list that input is checked against.
//!
//! Two handler families exist, and nothing else may be bound:
//!
//! | Family | What it is |
//! |--------|------------|
//! | [`Handler::VmBacked`] | the generic in-guest runner ([`proof_rlm::VmBackedRunner`]) — the Firecracker path |
//! | [`Handler::Harbor`] | an operator-baked Harbor adaptor, i.e. a `VmBacked` runner whose adaptor directory ships the Harbor harness |
//!
//! The distinction is *documentation and audit*, not a second code path: both
//! resolve to the same `VmBackedRunner` over the topic-VM orchestrator, and
//! neither can be a path, a URL, or a shell command. What the allow-list
//! prevents is an RLM section naming something like
//! `/bin/sh -c 'curl … | sh'`, an absolute path, or an arbitrary binary: those
//! are refused by shape before anything is bound, and the refusal names why.

use proof_canon::is_custom_id;

use crate::InstallError;

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

/// Handler names an RLM section may use, mapped to their family.
///
/// The left-hand names are what a bundle writes; the right-hand family is
/// what the install binds. Only these two spellings (plus their documented
/// synonyms) are accepted, and every one of them resolves to a `VmBacked`
/// runner — the allow-list is closed, so a name that is not here is refused
/// rather than defaulted.
pub const ALLOWED_HANDLERS: [(&str, Handler); 4] = [
    ("vm_backed", Handler::VmBacked),
    ("vm_backed_runner", Handler::VmBacked),
    ("harbor", Handler::Harbor),
    ("harbor_trials", Handler::Harbor),
];

/// Why a handler name was refused, in the terms the bundle wrote.
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
        /// What the bundle named.
        got: String,
    },
    /// The name is shaped like a path, a URL, or a command rather than an id.
    #[error(
        "handler {got:?} is not an identifier: a handler is a name this build resolves to a \
         baked adaptor, never a path, a URL, or a command line"
    )]
    NotAnIdentifier {
        /// What the bundle named.
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

/// Resolve a handler name from an RLM section.
///
/// Case and surrounding whitespace are tolerated, matching how the rest of
/// the CLI's operator inputs parse (`InstallEnvironment` does the same). That
/// tolerance cannot widen the allow-list: a path or a command line is still
/// refused as *not an identifier* after folding, because the fold only
/// touches case.
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

/// Resolve a handler for an install, mapping a refusal onto the install error.
///
/// # Errors
///
/// [`InstallError::HandlerNotAllowed`].
pub fn check_handler(name: &str) -> Result<Handler, InstallError> {
    resolve_handler(name).map_err(|e| InstallError::HandlerNotAllowed(e.to_string()))
}

/// The runner id an install binds for a topic.
///
/// The **signed document** is the source of truth: when it selects an
/// in-guest runner, that is the runner, and a bundle that names a different
/// one is a contradiction (the CLI refuses those before reaching here — see
/// `proof_topic_bundle::BundleError::HostContradictsDocument`). When the
/// document selects none, the install binds the topic's registered custom id
/// through the generic runner, which is what the operator's
/// `PROOF_VM_RUNNER_CUSTOM_IDS` entry already does.
#[must_use]
pub fn bound_runner(
    document_runner: Option<&str>,
    handler: Option<Handler>,
) -> (Option<String>, Handler) {
    match (document_runner, handler) {
        (Some(runner), _) => (Some(runner.to_owned()), Handler::VmBacked),
        (None, handler) => (None, handler.unwrap_or(Handler::VmBacked)),
    }
}
