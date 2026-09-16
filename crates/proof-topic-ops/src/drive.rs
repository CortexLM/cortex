//! Driving the topic's RLM setup from the operator CLI.
//!
//! `topic install --drive-rlm` runs the **real** [`TopicSetup`] over the
//! topic-VM orchestrator: it asks the RLM inside its own VM to provision, to
//! write its rules, and — unless `--skip-baseline` — to measure a baseline.
//! This is the same driver the challenge service uses; the CLI does not have
//! a second, weaker path.
//!
//! # What it needs, and why each is a gate
//!
//! | Need | Why |
//! |------|-----|
//! | `PROOF_VM_ORCHESTRATOR_URL` + token file + `PROOF_RLM_VM_IMAGE_DIGEST` | the VM boundary; unwired → refuse, never a host fallback |
//! | an owner hook | the lifecycle asks the owner before provisioning; the CLI's hook is the operator's `--owner-approved` |
//! | an owner key probe | `awaiting_owner_keys` advances only when the key file is present |
//! | a live `InferenceOffer` | the baseline is a paid run and needs a judge offer |
//! | `--owner-approved` | the flag that asserts an Owner authorized the VM and the spend |
//!
//! Every one of those is checked **before** the first job is forwarded, and a
//! missing piece stops the driver where it is with the reason. Nothing here
//! falls back to running on the control-plane host.

use std::path::Path;
use std::sync::Arc;

use proof_rlm::{
    FileKeysProbe, OwnerDecision, OwnerHook, OwnerKeysProbe, StaticOwnerHook, VmError,
    RLM_VM_IMAGE_DIGEST_ENV, VM_ORCHESTRATOR_TOKEN_FILE_ENV, VM_ORCHESTRATOR_URL_ENV,
};
use proof_rlm_store::{PgRlmStore, RlmStore};
use proof_task::{InferenceOffer, ProofPin, TopicDocument};
use proof_topic_setup::{SetupError, SetupOutcome, TopicSetup};

use crate::OpsError;

/// What driving the RLM produced, for the install report and the operator.
pub struct DriveOutcome {
    /// Rule version the RLM wrote.
    pub rules_version: u32,
    /// The baseline primary, when one was measured.
    pub baseline_primary: Option<f64>,
    /// The topic's VM id.
    pub vm_id: String,
    /// The lifecycle state the driver left the topic in.
    pub state: String,
    /// The whole set the RLM authored — the topic's behavior, which the
    /// install applies in place of the bundle's section.
    ///
    /// `None` means the run returned a bare rule vector. The drive **refuses**
    /// before returning in that case ([`SetupError::IncompleteAuthoring`]),
    /// so an outcome with `authored: None` is only reachable from a run that
    /// stopped earlier — and the install then falls back to the operator's
    /// section, whose `topic_document` provenance the publish gate refuses to
    /// open a topic on.
    pub authored: Option<Box<proof_topic_authoring::TopicAuthoring>>,
}

impl DriveOutcome {
    /// One-line summary for the operator.
    #[must_use]
    pub fn summary(&self) -> String {
        let parts = match &self.authored {
            Some(set) => format!(
                ", authoring {} ({} migrations, {} apis, pin policy {})",
                set.digest(),
                set.migrations.len(),
                set.apis.len(),
                if set.pin_policy.is_empty() {
                    "none"
                } else {
                    "set"
                }
            ),
            None => ", no authored set".to_owned(),
        };
        match self.baseline_primary {
            Some(v) => format!(
                "the RLM wrote rules v{} and measured a baseline of {v} on vm {}{parts}",
                self.rules_version, self.vm_id
            ),
            None => format!(
                "the RLM wrote rules v{} on vm {} (no baseline: --skip-baseline){parts}",
                self.rules_version, self.vm_id
            ),
        }
    }
}

/// Drive the RLM setup for `topic`.
///
/// Every parameter is a piece of host configuration the driver must check
/// **before** it forwards the first job, and each one is named in the refusal
/// it produces — so they are explicit here rather than bundled into a config
/// struct a caller could half-fill.
///
/// # Errors
///
/// [`OpsError::usage`](crate::OpsError::usage) for a missing piece of host
/// configuration (naming the env var), [`OpsError::error`](crate::OpsError::error)
/// for a refusal from the orchestrator, the
/// lifecycle, or the store.
#[allow(clippy::too_many_arguments)]
pub async fn drive(
    topic: &TopicDocument,
    pin: &ProofPin,
    store: PgRlmStore,
    skip_baseline: bool,
    owner_approved: bool,
    orchestrator_url: Option<&str>,
    orchestrator_token_file: Option<&Path>,
    rlm_image_digest: Option<&str>,
    offer: Option<InferenceOffer>,
    owner_key_file: Option<&Path>,
) -> Result<DriveOutcome, OpsError> {
    if !owner_approved {
        return Err(OpsError::usage(
            "driving the RLM provisions a topic VM and runs a paid baseline, so it requires \
             --owner-approved."
                .to_owned(),
        ));
    }
    // The VM boundary. `FirecrackerOrchestrator::from_env` is the same reader
    // the challenge service uses; the CLI does not re-implement it, so a host
    // wired for scoring is wired for install and vice versa.
    let orchestrator =
        resolve_orchestrator(orchestrator_url, orchestrator_token_file, rlm_image_digest)?;
    // A baseline is a paid run: without an offer there is nothing to measure
    // against. `--skip-baseline` is the path that does not need one.
    if offer.is_none() && !skip_baseline {
        return Err(OpsError::usage(
            "the RLM's baseline is a paid run that needs a live judge offer: set \
             PROOF_INFERENCE_OFFER_FILE to an open InferenceOffer (or pass --skip-baseline to \
             install the rules without measuring one). Without a baseline the topic cannot open \
             anyway."
                .to_owned(),
        ));
    }
    // The lifecycle asks the owner before provisioning and probes for the key
    // file before it advances. The CLI's hook is the operator's own assertion
    // (`--owner-approved`), which the caller has already checked; the probe is
    // the real file check, so a topic that cannot reach its key stops rather
    // than provisioning.
    let keys = resolve_keys(owner_key_file);
    let owner: Arc<dyn OwnerHook> = Arc::new(StaticOwnerHook(OwnerDecision::Approve));
    let setup = TopicSetup {
        orchestrator,
        store: Arc::new(store) as Arc<dyn RlmStore>,
        template: proof_rlm::VmTemplate::from_env(),
        experiments: proof_rlm::ExperimentPolicy::from_env().map_err(|e| {
            OpsError::usage(format!(
                "the per-experiment VM policy is malformed: {e}. Fix the \
                 PROOF_EXPERIMENT_VM_* env before driving the RLM."
            ))
        })?,
        owner,
        keys,
        spend_cap_usd: None,
        skip_baseline,
    };
    // `offer` is required unless `skip_baseline`: the baseline is the only
    // consumer, so a skipping run never needs one and is never handed a
    // placeholder.
    let outcome = setup
        .run(topic, pin, offer.as_ref())
        .await
        .map_err(|e| OpsError::error(drive_failure(&e)))?;
    Ok(outcome_summary(outcome))
}

/// The live orchestrator, or a refusal naming what is missing.
fn resolve_orchestrator(
    url: Option<&str>,
    token_file: Option<&Path>,
    image_digest: Option<&str>,
) -> Result<Arc<dyn proof_rlm::TopicVmOrchestrator>, OpsError> {
    // Presence only: `FirecrackerOrchestrator::from_env` reads the env itself,
    // so the CLI checks that each piece *is* set (and names the missing one)
    // without duplicating the client's parsing and validation.
    if url.map(str::trim).is_none_or(str::is_empty) {
        return Err(OpsError::usage(format!(
            "driving the RLM needs the topic-VM orchestrator: set {VM_ORCHESTRATOR_URL_ENV} \
             (https, the KVM host agent) plus {VM_ORCHESTRATOR_TOKEN_FILE_ENV} and \
             {RLM_VM_IMAGE_DIGEST_ENV}. Nothing is driven on the control-plane host — that is the \
             boundary, not a fallback."
        )));
    }
    if token_file.is_none() {
        return Err(OpsError::usage(format!(
            "driving the RLM needs {VM_ORCHESTRATOR_TOKEN_FILE_ENV}: a file holding the bearer \
             for the topic-VM orchestrator. It is re-read per request and never logged."
        )));
    }
    if image_digest.map(str::trim).is_none_or(str::is_empty) {
        return Err(OpsError::usage(format!(
            "driving the RLM needs {RLM_VM_IMAGE_DIGEST_ENV}: the sha256 digest of the RLM VM \
             image the orchestrator boots. A digest is never invented."
        )));
    }
    match proof_vm_fc::FirecrackerOrchestrator::from_env() {
        Ok(Some(fc)) => Ok(Arc::new(fc)),
        Ok(None) => Err(OpsError::usage(format!(
            "no topic-VM orchestrator resolved from {VM_ORCHESTRATOR_URL_ENV} / \
             {VM_ORCHESTRATOR_TOKEN_FILE_ENV} / {RLM_VM_IMAGE_DIGEST_ENV}"
        ))),
        Err(e) => Err(OpsError::usage(format!(
            "the topic-VM orchestrator configuration was refused: {e}"
        ))),
    }
}

/// The owner key probe: the file if given, else the env-configured one.
fn resolve_keys(owner_key_file: Option<&Path>) -> Arc<dyn OwnerKeysProbe> {
    match owner_key_file {
        Some(path) => Arc::new(FileKeysProbe::new(path)),
        None => match FileKeysProbe::from_env() {
            Some(probe) => Arc::new(probe),
            None => Arc::new(NoKeys),
        },
    }
}

/// A probe that always refuses, naming what to set.
///
/// The lifecycle stops at `awaiting_owner_keys` rather than proceeding, which
/// is the fail-closed direction: a topic that cannot reach its owner key is
/// not provisioned.
struct NoKeys;

impl OwnerKeysProbe for NoKeys {
    fn owner_keys_present(&self) -> Result<(), proof_rlm::HookError> {
        Err(proof_rlm::HookError::Failed(format!(
            "no owner key file configured: set {} (or pass --owner-key-file) to the file \
             holding the owner's inference key",
            proof_rlm::OWNER_INFERENCE_KEY_FILE_ENV
        )))
    }
}

/// Turn a setup refusal into an operator instruction.
fn drive_failure(err: &SetupError) -> String {
    let base = format!("driving the RLM stopped: {err}");
    let guidance = match err {
        SetupError::Declined(_) => {
            "The owner declined, so the topic is back at draft and nothing was provisioned."
        }
        SetupError::State(proof_rlm::StateError::KeysMissing(_)) => {
            "The lifecycle stopped at `awaiting_owner_keys`: the owner key file is missing or \
             empty. Provide it and re-run; the driver resumes from the persisted state."
        }
        SetupError::Vm(VmError::NotWired(_)) => {
            "The topic-VM orchestrator is not wired on this host. Fix the env and re-run; nothing \
             ran on the control-plane host."
        }
        SetupError::Vm(_) => {
            "The orchestrator refused or the job failed. The lifecycle is left where it stopped, \
             so a re-run resumes rather than restarts. A failed baseline run retains its guest on \
             the KVM host for root-cause analysis."
        }
        SetupError::NotCustom(_) => {
            "Only a custom-family topic has an RLM to drive; nothing else was changed."
        }
        _ => "The lifecycle is left where it stopped (persisted), so a re-run resumes.",
    };
    format!(
        "{base}\n  {guidance}\n  The install journal still records what the static half applied; \
         `proof-admin topic install-log --topic <id>` reads it back."
    )
}

/// Summarize what the driver returned.
fn outcome_summary(outcome: SetupOutcome) -> DriveOutcome {
    let measured = outcome.measured_baseline();
    DriveOutcome {
        rules_version: outcome.rules_version,
        baseline_primary: outcome.baseline_primary,
        vm_id: outcome.vm.vm_id,
        state: if measured {
            "baselining (the operator seals next)".to_owned()
        } else {
            "baselining (no baseline measured: --skip-baseline)".to_owned()
        },
        authored: outcome.authored,
    }
}
