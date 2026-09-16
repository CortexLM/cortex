//! The operator-facing surface of the Proof HTTP API: the journal the submit
//! path reads, and the topic-VM orchestrator diagnostic.
//!
//! These are the types a **host** implements (the challenge binary reads the
//! shared database; the VM client answers the orchestrator probe) and the
//! routes consume. They live in their own module rather than in the router
//! file because the router is at the repository's per-crate LOC cap, and
//! because their subject is a boundary: what the control plane is allowed to
//! *ask* about a topic's install and its VM host.

use std::sync::Arc;

use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use crate::{err, AppState, ErrResp};

/// The KVM-host agent's health, as the agent's own `GET /v1/health` reports it.
///
/// Re-exported from `proof-vm-proto` rather than mirrored: the route publishes
/// what the agent said, so a second struct here could only drift from the wire
/// type it copies.
pub use proof_vm_proto::AgentHealth as VmAgentHealth;

/// `GET /v1/admin/proof/vm-orchestrator` body: what this host resolved for
/// the topic-VM orchestrator and whether its agent answers. Operator data
/// behind the admin bearer — it may name env vars and container paths, never
/// the bearer, a key, or an origin the RLM could reach.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmOrchestratorReport {
    /// `firecracker` (live client resolved at boot) or `unwired`.
    pub orchestrator: String,
    /// The client's own `ready()`: bearer file present and non-empty, RLM
    /// image digest pinned. Checked per request, so a fix needs no restart.
    pub ready: bool,
    /// Why not ready (empty when ready). Names the env var to fix.
    pub reason: String,
    /// `sha256:` pin of the RLM VM image the client asks the agent to boot
    /// (empty = unpinned = nothing ever boots).
    pub image_digest: String,
    /// RLM VM vCPUs (locked default 4).
    pub vcpus: u32,
    /// RLM VM memory in MiB (locked default 8192).
    pub mem_mib: u32,
    /// The agent's answer to one health call, when it answered.
    pub agent: Option<VmAgentHealth>,
    /// Why the agent did not answer: unreachable, bearer refused, not wired.
    pub agent_error: Option<String>,
    /// Filled by the host: the digest-pinned Lium harvest (`nll` /
    /// `throughput`) is wired. Lium only — informational for the custom
    /// family, which is wired from the topic-VM env on its own.
    #[serde(default)]
    pub live_harvest_wired: bool,
    /// Filled by the host: at least one custom id has a registered runner
    /// (the custom family is routed, harvest or not).
    #[serde(default)]
    pub custom_family_wired: bool,
    /// Filled by the host: custom ids with a registered runner.
    #[serde(default)]
    pub registered_custom: Vec<String>,
}

impl VmOrchestratorReport {
    /// Report for a host that keeps `UnwiredVmOrchestrator`; `reason` names
    /// the env vars a live one reads. `image_digest` is whatever pin the env
    /// carries so "pinned but URL unset" is visible.
    #[must_use]
    pub fn unwired(reason: &str, image_digest: &str) -> Self {
        Self {
            orchestrator: "unwired".into(),
            ready: false,
            reason: reason.trim().to_owned(),
            image_digest: image_digest.trim().to_owned(),
            vcpus: 0,
            mem_mib: 0,
            agent: None,
            agent_error: None,
            live_harvest_wired: false,
            custom_family_wired: false,
            registered_custom: Vec::new(),
        }
    }
}

/// Operator diagnostic over the topic-VM orchestrator this host resolved at
/// boot. The binary implements it over the live `FirecrackerOrchestrator`
/// (its `ready()` plus one agent health call) or the unwired stand-in; the
/// route only adds what the host knows (harvest wired, registered ids). It
/// changes nothing and spends nothing.
#[async_trait::async_trait]
pub trait VmOrchestratorProbe: Send + Sync {
    /// Snapshot as of now (bearer file and pin re-read; one agent round trip).
    async fn probe(&self) -> VmOrchestratorReport;
}

/// What the **submit** path needs from the journal, in one read.
///
/// Both fields are about the topic's install and the operator's switch, and
/// both are read before anything is spent, so they are one call: a submission
/// costs one gate read, not two.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubmitGate {
    /// The operator's reason, when the topic is disabled (`topic disable`).
    pub disabled_reason: Option<String>,
    /// The `vms_per_submission` the topic's newest install recorded.
    ///
    /// `None` when the topic has no install row or the row predates the
    /// field. `Some(n)` with `n != 1` is refused on the submit path: this
    /// build runs **one VM per submission**, and a topic installed with a
    /// different pin is one it cannot honour.
    pub vms_per_submission: Option<u32>,
}

impl SubmitGate {
    /// The operator gate, as the submit path reads it.
    #[must_use]
    pub fn disabled(reason: impl Into<String>) -> Self {
        Self {
            disabled_reason: Some(reason.into()),
            ..Self::default()
        }
    }
}

/// Whether a topic is ready to be published `open`, as the publish route
/// reads it.
///
/// A trait rather than a pool so the route can be exercised without a
/// database, and so a host that resolved no install journal can say so instead
/// of answering from a table it never read.
#[async_trait::async_trait]
pub trait InstallJournal: Send + Sync {
    /// `Ok(true)` when the topic's install reached `applied` **and** its rule
    /// vector in force is RLM-authored.
    ///
    /// Two facts, because an `open` document is submitable the moment it is
    /// published: the install has to be in place, and the topic's behavior has
    /// to have been authored by its own RLM rather than still being the
    /// operator's signed `checklist` (`proof_rule_version.source`).
    ///
    /// # Errors
    ///
    /// The reason the journal could not be read. The caller refuses the
    /// publish: an unreadable journal is not an installed topic.
    async fn applied(&self, topic_id: &str) -> Result<bool, String>;
    /// The operator gate and the allocator pin for one topic, for the submit
    /// path.
    ///
    /// # Errors
    ///
    /// The reason the gate could not be read. The caller answers **503**: an
    /// unreadable gate is not an enabled topic.
    async fn submit_gate(&self, topic_id: &str) -> Result<SubmitGate, String>;

    /// Every topic currently disabled, with the operator's reason.
    ///
    /// One read for the public listing, which annotates each topic with the
    /// flag rather than paying a query per topic. The submit path uses
    /// [`Self::submit_gate`] for the single topic it is admitting.
    ///
    /// # Errors
    ///
    /// The reason the gate could not be read. The caller answers **503**.
    async fn disabled_topics(&self) -> Result<std::collections::BTreeMap<String, String>, String>;
}

/// The journal read behind the publish gate, or `None` on a host that
/// resolved none (no database).
///
/// `None` is **fail-closed**: [`install_gate`] refuses, so an `open`
/// document cannot be published on a host that cannot prove the install ran.
/// That is the same rule the gate enforces when the journal is unreadable —
/// the only difference is which sentence the operator reads.
pub type InstallJournalSlot = Option<Arc<dyn InstallJournal>>;

/// Whether the topic is ready to be published `open`, as the publish gate
/// reads it.
///
/// **Fail-closed on every doubt**: no journal slot, an unreadable journal, a
/// topic with no install row, and a topic whose rules are not RLM-authored all
/// refuse, so an `open` document is never published before its migrations,
/// routes, and rules are in place *and* its RLM has authored its behavior.
pub(crate) async fn install_gate(st: &AppState, topic_id: &str) -> Result<(), String> {
    let Some(journal) = st.install_journal.as_deref() else {
        return Err(format!(
            "this host resolved no install journal (no database), so it cannot prove that topic \
             {topic_id:?} was installed. Publish the document as `draft`, or wire \
             BASE_DATABASE_URL and restart."
        ));
    };
    match journal.applied(topic_id).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(format!(
            "topic {topic_id:?} is not ready to be `open`: either its newest `proof_topic_install` \
             row is not `applied`, or its rule vector is not RLM-authored \
             (`proof_rule_version.source` is not `rlm`). Run `proof-admin topic install` \
             --drive-rlm --owner-approved to completion, then read `proof-admin topic \
             install-log --topic {topic_id}`. Rules still sourced from the signed document mean \
             the topic's behavior was not authored by its RLM."
        )),
        Err(e) => Err(format!(
            "the install journal could not be read for topic {topic_id:?}: {e}. The publish is \
             refused rather than admitted on an unread fact; fix the database and re-publish."
        )),
    }
}

/// The operator gate on the **submit** path: is this topic disabled, and is
/// its allocator pin one this build runs?
///
/// `Ok(())` admits the submission. A disabled topic is a **403** naming the
/// operator's reason; a topic whose install recorded a `vms_per_submission`
/// other than 1 is a **503** — this build runs exactly one VM per submission,
/// and silently running a different number would make the journal a lie; an
/// unreadable gate is a **503** too — never an admission, and never a 404 that
/// would read as "no such topic". A host that resolved no journal (no
/// database) has no gate to read: it also has no published topics, so the
/// submission is refused by the topic lookup above it.
///
/// This is deliberately **not cached**: a disable has to take effect on the
/// next request, which is what makes it usable during an incident.
pub(crate) async fn disabled_gate(st: &AppState, topic_id: &str) -> Result<(), ErrResp> {
    let Some(journal) = st.install_journal.as_deref() else {
        return Ok(());
    };
    let gate = match journal.submit_gate(topic_id).await {
        Ok(gate) => gate,
        Err(e) => {
            return Err(err(
                StatusCode::SERVICE_UNAVAILABLE,
                &format!(
                    "the topic gate could not be read for {topic_id:?}: {e}. The submission is \
                     refused rather than admitted on an unread fact; fix the database and re-post \
                     (the submit_nonce is unspent)."
                ),
            ))
        }
    };
    if let Some(reason) = gate.disabled_reason.as_deref() {
        return Err(err(
            StatusCode::FORBIDDEN,
            &format!(
                "topic {topic_id:?} is disabled by the operator{}",
                if reason.trim().is_empty() {
                    String::new()
                } else {
                    format!(": {}", reason.trim())
                }
            ),
        ));
    }
    if let Some(n) = gate.vms_per_submission {
        if n != proof_topic_install::VMS_PER_SUBMISSION {
            return Err(err(
                StatusCode::SERVICE_UNAVAILABLE,
                &format!(
                    "topic {topic_id:?} was installed with vms_per_submission={n}, but this host \
                     runs exactly {} VM per submission (the pin this build enforces). Re-install \
                     the topic with the pin this host carries, or run the host that matches the \
                     install. The submission is refused rather than run under a binding the \
                     install did not record; the submit_nonce is unspent.",
                    proof_topic_install::VMS_PER_SUBMISSION
                ),
            ));
        }
    }
    Ok(())
}

/// The disabled set the listing annotates from, or a 503.
pub(crate) async fn disabled_topics(
    st: &AppState,
) -> Result<std::collections::BTreeMap<String, String>, ErrResp> {
    let Some(journal) = st.install_journal.as_deref() else {
        // No gate on this host: no database, so no operator switch was ever
        // thrown (and no topic was published either).
        return Ok(std::collections::BTreeMap::new());
    };
    journal.disabled_topics().await.map_err(|e| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            &format!(
                "the topic gate could not be read: {e}. The listing is refused rather than \
                 served without the operator's switch; fix the database and re-read."
            ),
        )
    })
}
