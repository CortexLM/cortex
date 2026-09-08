//! Proof RLM engine — host side. No challenge content.
//!
//! - [`RlmScorer`] is the `LiveScorer` for the whole `custom` metric family:
//!   resolve the topic's `custom_id` in the runner registry (unknown →
//!   `RunnerUnwired`, 503, no row), load the topic's current rule version
//!   from the store, inspect → checklist (persisted), red → reject without
//!   paid inference, green → spend token → evaluate → `custom_value` with
//!   the runner-measured `flops_used` in the verdict. It is wired through
//!   `proof_eval::FamilyMux` so no custom topic ever falls back to the
//!   digest-pinned harvest. Runs hold a per-topic lease from `score` until
//!   the row is persisted, so promotion is decided and written against the
//!   store's current best and a worse run can never displace a champion.
//! - [`ArtefactStore`] writes `{root}/{topic_id}/{submission_id}.zip`
//!   (`manifest.json`, `artifact/`, `report.json`, `checklist.json`,
//!   `baseline_ref.json`, `logs/`) once the row is persisted, `best.json`
//!   on promotion, and `events.jsonl` (public events) — with metadata and
//!   the promotion continuum mirrored into the RLM store.
//! - [`TopicSetup`] drives `draft → … → baselining` over the topic-VM
//!   boundary (owner hook, key probe, provision, RLM rule proposal →
//!   store, baseline → store), and `mark_sealed` closes the loop to `open`.
//!
//! Core types live in `proof-rlm`; persistence in `proof-rlm-store`.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::too_many_arguments
)]

mod artefact;
mod scorer;
mod setup;

pub use artefact::{
    artefact_path, is_safe_entry_path, ArtefactBundle, ArtefactError, ArtefactManifest,
    ArtefactStore, BaselineRef, BestRef, PublicEvent, WrittenArtefact, ARTEFACT_MANIFEST_SCHEMA,
    ARTEFACT_ROOT_ENV, ARTIFACT_DIR, BASELINE_REF_FILE, BEST_FILE, CHECKLIST_FILE,
    DEFAULT_ARTEFACT_ROOT, EVENTS_FILE, LOGS_DIR, MANIFEST_FILE, MAX_ENTRY_PATH, REPORT_FILE,
};
pub use scorer::{RlmScorer, DEFAULT_LEASE_TTL};
pub use setup::{SetupError, SetupOutcome, TopicSetup};
