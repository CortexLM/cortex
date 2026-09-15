//! The Proof operator **procedures** that reach a live host: drive the RLM
//! setup, read the baseline it measured, seal it, and publish the open
//! document.
//!
//! `proof-admin` is the CLI; this crate is what its subcommands *do*. The
//! split exists because the binary is at the repository's per-crate LOC cap,
//! and because these procedures have a different subject from argument
//! parsing: every one of them either provisions something (a VM, a paid
//! baseline), writes durable state (the seal, the lifecycle move), or calls a
//! master over the network — the things an operator needs to be able to read
//! in one place.
//!
//! | Procedure | What it reaches |
//! |-----------|-----------------|
//! | [`drive`] | the topic-VM orchestrator: provision → `propose_rules` → baseline |
//! | [`baseline`] | the shared database: the measurement the RLM stored |
//! | [`seal`] | `TopicSetup::mark_sealed`, then the admin publish route |
//!
//! Nothing here holds a signing key: the `proof` topic key stays with the
//! operator, and `xtask proof-topic` is what signs a document. Nothing here
//! falls back to the control-plane host: an unwired orchestrator is a
//! refusal, and a missing owner assertion is a refusal, both named.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::module_name_repetitions
)]

pub mod drive;
pub mod publish;
pub mod seal;

pub use drive::{drive, DriveOutcome};
pub use publish::PublishTarget;
pub use seal::{baseline, lifecycle, seal, BaselineReport, LifecycleReport, SealArgs, SealOutcome};

/// Why an operator procedure refused.
///
/// Two kinds, because the CLI's exit codes depend on the difference: a
/// **usage** error is a flag or an environment variable the operator can fix
/// before re-running, and anything else is a refusal from a host or the
/// database. Keeping them distinct here is what lets the binary stay a thin
/// adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpsError {
    /// Bad usage or missing configuration: the operator fixes a flag or an
    /// env var. The CLI exits 2.
    Usage(String),
    /// A refusal from a host, the database, or a document. The CLI exits 1.
    Error(String),
}

impl std::fmt::Display for OpsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage(m) | Self::Error(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for OpsError {}

impl OpsError {
    /// A usage refusal.
    #[must_use]
    pub fn usage(message: impl Into<String>) -> Self {
        Self::Usage(message.into())
    }

    /// Any other refusal.
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self::Error(message.into())
    }

    /// Whether this is the operator's usage to fix.
    #[must_use]
    pub const fn is_usage(&self) -> bool {
        matches!(self, Self::Usage(_))
    }
}
