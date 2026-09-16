//! `topic seal` and `topic baseline` — the step between "the RLM measured a
//! baseline" and "the topic is scorable".
//!
//! `proof-admin topic install --drive-rlm` (without `--skip-baseline`) leaves
//! the topic at **`baselining`** with a measured baseline in
//! `proof_baseline_measurement`. Nothing is scorable yet: the signed document
//! is still the bundle's `draft`, and a `draft` takes no submissions. The
//! remaining ceremony is the operator's, and it is two commands:
//!
//! ```text
//! # 1. What did the RLM measure, and what must the open document seal?
//! proof-admin topic baseline <topic-id>
//!
//! # 2. Sign the open document carrying that commitment, then:
//! proof-admin topic seal <topic-id> --document open.json --publish \
//!   --admin-url https://<gateway> --admin-token-file /run/base/proof/admin_tokens
//! ```
//!
//! [`baseline`] is the read half: the measured primary, the rule version it
//! was measured under, and the `metrics_commitment` an `open` document must
//! carry — the value that goes into the draft before it is signed (this crate
//! never signs: the `proof` key stays with the operator, and `xtask
//! proof-topic` is what signs a draft).
//!
//! [`seal`] is the write half, and it is deliberately thin: it hands the
//! signed document and the measurement to [`TopicSetup::mark_sealed`], the
//! **same** call the challenge's own tests drive, so the CLI cannot seal
//! something the runtime would refuse. `mark_sealed` is what checks, in
//! order: the document is `status: open`; it validates as an open topic on
//! this host (a registered custom id, a sealed baseline, tighten-only
//! floors); its signature verifies under the pin's topic key; the sealed
//! measurement binds to the document and its `custom_value` is the primary
//! the RLM actually measured in the topic VM; and the lifecycle is at
//! `baselining`. Only then does the topic move to `open`, and only then can
//! `--publish` reach the admin route — which itself refuses an `open`
//! document whose install is not `applied`.
//!
//! # Why the measurement is built here, and not handed in
//!
//! A `BaselineMeasurement` is the seal: it carries the eval image digest, the
//! topic's holdout commitment, the metric vector, and the primary. For a
//! custom-family topic the primary is `custom_value` (the NLL fields are not
//! this family's metric — they are zero, which is the shape the RLM's own
//! e2e drives). Building it from the **stored** measurement rather than from
//! an operator-supplied file is what makes "the sealed value is the value the
//! RLM measured" a property of the command instead of a promise: the number
//! comes from `proof_baseline_measurement`, and `mark_sealed` compares it
//! against the document.

use std::path::Path;
use std::sync::Arc;

use proof_eval::BaselineMeasurement;
use proof_rlm_store::{BaselineRow, PgRlmStore, RlmStore};
use proof_task::{HoldoutSplit, ProofPin, TopicDocument, TopicStatus};
use proof_topic_setup::TopicSetup;

use crate::publish::PublishTarget;
use crate::OpsError;

/// Everything `topic seal` was asked to do.
pub struct SealArgs<'a> {
    /// Topic slug, or an alias of one.
    pub topic_id: &'a str,
    /// The signed `status: open` document.
    pub document: &'a Path,
    /// Pin the document is checked against.
    pub pin: &'a ProofPin,
    /// Publish the sealed document through the admin route.
    pub publish: bool,
    /// Master base URL for the publish call (with `--publish`).
    pub admin_url: Option<&'a str>,
    /// File holding the operator bearer (with `--publish`).
    pub admin_token_file: Option<&'a Path>,
    /// Custom ids this host registers for scoring
    /// (`PROOF_VM_RUNNER_CUSTOM_IDS`): an open custom topic needs one.
    pub registered_custom: Vec<String>,
}

/// What the RLM measured, and the commitment an `open` document must seal.
#[derive(Debug, Clone, PartialEq)]
pub struct BaselineReport {
    /// Canonical topic slug (the alias resolved).
    pub topic_id: String,
    /// Rule version the baseline was measured under.
    pub rules_version: u32,
    /// The measured primary — what the document's `custom_value` seals.
    pub primary_value: f64,
    /// The document's metric primary name.
    pub metric_primary: String,
    /// The document's custom id.
    pub custom_id: String,
    /// The topic's holdout commitment.
    pub holdout_commitment: String,
    /// The `baseline.metrics_commitment` an open document must carry.
    pub metrics_commitment: String,
    /// The document's current status (a draft is not scorable).
    pub document_status: TopicStatus,
    /// Whether this measurement is a **degenerate bar**: a relative-win family
    /// whose measured primary is ~zero, so no challenger could ever clear it
    /// and sealing it will be refused (`SetupError::DegenerateBar`).
    ///
    /// Surfaced here because this is the read that happens **before** the
    /// operator signs an `open` document: warning at seal time is correct but
    /// late — the document would already carry a number that cannot be sealed.
    pub degenerate_bar: bool,
}

impl BaselineReport {
    /// The steps that turn this measurement into a scorable topic.
    #[must_use]
    pub fn next_steps(&self) -> String {
        next_seal_steps(&self.topic_id, &self.metrics_commitment)
    }
}

/// What sealing produced.
#[derive(Debug, Clone, PartialEq)]
pub struct SealOutcome {
    /// Canonical topic slug.
    pub topic_id: String,
    /// The document version now stored (unchanged on a retry).
    pub document_version: u32,
    /// The primary the RLM measured and the document sealed.
    pub primary_value: f64,
    /// The commitment the document carries.
    pub metrics_commitment: String,
    /// Whether the open document was published through the admin route.
    pub published: bool,
    /// The seal was already recorded (this run only published).
    ///
    /// The retry path: the previous run sealed the topic and the publish
    /// failed, so the operator re-runs the same command and this one skips
    /// `mark_sealed` rather than being refused by the lifecycle it moved.
    pub already_sealed: bool,
}

impl SealOutcome {
    /// What to do next, for the operator.
    #[must_use]
    pub fn after(&self) -> String {
        after_seal(&self.topic_id, self.published)
    }
}

/// Whether a measured primary is a bar no challenger could ever clear.
///
/// The same predicate `mark_sealed` refuses on, named once so the early read
/// (`topic baseline`) and the seal cannot disagree about what "degenerate"
/// means. A missing primary is not degenerate — it is missing evidence, which
/// the scoring gate reports as such.
fn baseline_is_degenerate(document: &TopicDocument, primary_value: f64) -> bool {
    proof_score::family_bar_is_degenerate(document.metric.family, Some(primary_value))
}

/// `topic baseline`: what the RLM measured, and what to seal.
///
/// # Errors
///
/// [`OpsError::error`] when the topic or its measurement cannot be read.
pub async fn baseline(
    pool: &sqlx::PgPool,
    pin: &ProofPin,
    topic_id: &str,
) -> Result<BaselineReport, OpsError> {
    let store = PgRlmStore::new(pool.clone());
    let (canonical, _, document) = resolve_topic(&store, topic_id).await?;
    let Some(measured) = store
        .baseline(&canonical)
        .await
        .map_err(|e| OpsError::error(format!("{canonical} baseline: {e}")))?
    else {
        return Err(OpsError::error(format!(
            "no baseline measured for topic {canonical:?}. It is written by the RLM's baseline \
             job: run `proof-admin topic install --bundle <bundle> --env <target> --drive-rlm \
             --owner-approved` (without --skip-baseline) first. Nothing to seal yet."
        )));
    };
    let commitment = seal_measurement(pin, &document, &measured).commitment();
    Ok(BaselineReport {
        topic_id: canonical,
        rules_version: measured.rules_version,
        primary_value: measured.primary_value,
        metric_primary: document.metric.primary.clone(),
        custom_id: document.metric.custom_id.clone(),
        holdout_commitment: document.holdout_commitment.clone(),
        metrics_commitment: commitment,
        document_status: document.status,
        degenerate_bar: baseline_is_degenerate(&document, measured.primary_value),
    })
}

/// `topic seal`: record the operator's seal and open the topic.
///
/// # Errors
///
/// [`OpsError::usage`] for a document that is not this topic's or is not
/// `open`, [`OpsError::error`] for a missing measurement, a lifecycle that is
/// not at `baselining`, a refused document, or a refused publish.
pub async fn seal(pool: &sqlx::PgPool, args: &SealArgs<'_>) -> Result<SealOutcome, OpsError> {
    let store = PgRlmStore::new(pool.clone());
    let (canonical, version, _) = resolve_topic(&store, args.topic_id).await?;
    let body = std::fs::read_to_string(args.document)
        .map_err(|e| OpsError::error(format!("read {}: {e}", args.document.display())))?;
    let document: TopicDocument = serde_json::from_str(&body)
        .map_err(|e| OpsError::error(format!("{}: {e}", args.document.display())))?;
    if document.id != canonical {
        return Err(OpsError::usage(format!(
            "{} carries topic {:?}, but this command is sealing {canonical:?}{}. Nothing was \
             changed.",
            args.document.display(),
            document.id,
            alias_note(args.topic_id, &canonical)
        )));
    }
    if document.status != TopicStatus::Open {
        return Err(OpsError::usage(format!(
            "{} is `{}`, not `open`. Sealing opens a topic, so the document has to be the open \
             one: set `status: open`, seal `baseline.metrics_commitment` from `proof-admin topic \
             baseline`, sign it, and re-run. Nothing was changed.",
            args.document.display(),
            status_word(document.status)
        )));
    }
    let Some(measured) = store
        .baseline(&canonical)
        .await
        .map_err(|e| OpsError::error(format!("{canonical} baseline: {e}")))?
    else {
        return Err(OpsError::error(format!(
            "no baseline measured for topic {canonical:?}, so there is nothing to seal. Run the \
             install with --drive-rlm (without --skip-baseline) first. Nothing was changed."
        )));
    };
    let sealed = seal_measurement(args.pin, &document, &measured);
    let commitment = sealed.commitment();
    // The one call that decides: the same `mark_sealed` the runtime's own
    // tests drive, so a document this command accepts is one the scoring path
    // would accept.
    //
    // **Unless the seal already landed.** `mark_sealed` moves the lifecycle
    // `baselining → open`, and the publish is a *remote* call that can fail
    // after it: the retry an operator is told to run (`--publish` again) would
    // otherwise be refused by the lifecycle it already moved. So a topic that
    // is already open under *this* document is not re-sealed — the seal is
    // recorded, and what remains is the publish. Anything else (an open topic
    // under a different document, or a lifecycle that never reached
    // `baselining`) still goes through `mark_sealed`, which is what refuses
    // it with the reason.
    let already_open = already_sealed(&store, &canonical, &document).await?;
    if !already_open {
        let registered: Vec<&str> = args.registered_custom.iter().map(String::as_str).collect();
        let setup = seal_setup(store);
        setup
            .mark_sealed(&document, args.pin, &registered, &sealed)
            .await
            .map_err(|e| OpsError::error(seal_failure(&e, &canonical)))?;
    }
    let document_version = if already_open { version } else { version + 1 };

    if args.publish {
        let admin = PublishTarget::resolve(args.admin_url, args.admin_token_file)?;
        admin
            .publish(&document)
            .await
            .map_err(|e| OpsError::error(publish_failure(&e, &canonical)))?;
    }
    Ok(SealOutcome {
        topic_id: canonical,
        document_version,
        primary_value: measured.primary_value,
        metrics_commitment: commitment,
        published: args.publish,
        already_sealed: already_open,
    })
}

/// Whether the topic is already open under **this** document and measurement.
///
/// The retry predicate, and deliberately narrow: the newest stored version
/// must be the document being sealed (same id, same signature, `open`) and
/// the lifecycle must already be `open`. A draft, a different signature, or a
/// lifecycle anywhere else answers `false`, so the caller still runs
/// `mark_sealed` and the operator still gets its refusal.
async fn already_sealed(
    store: &PgRlmStore,
    canonical: &str,
    document: &TopicDocument,
) -> Result<bool, OpsError> {
    let Some((_, latest)) = store
        .latest_topic(canonical)
        .await
        .map_err(|e| OpsError::error(format!("{canonical}: {e}")))?
    else {
        return Ok(false);
    };
    if latest.signature != document.signature || latest.status != TopicStatus::Open {
        return Ok(false);
    }
    let lifecycle = store
        .lifecycle(canonical)
        .await
        .map_err(|e| OpsError::error(format!("{canonical} lifecycle: {e}")))?;
    Ok(lifecycle.is_some_and(|lc| lc.state == proof_rlm::RlmState::Open))
}

/// The `TopicSetup` `mark_sealed` needs: the store, and a VM boundary that is
/// deliberately **unwired**.
///
/// Sealing touches no VM — it validates a signed document against a stored
/// measurement — so handing it a stub that refuses every call is the honest
/// wiring: if a future change made sealing provision something, it would stop
/// here instead of quietly running on the control-plane host.
fn seal_setup(store: PgRlmStore) -> TopicSetup {
    TopicSetup {
        orchestrator: Arc::new(proof_rlm::UnwiredVmOrchestrator),
        store: Arc::new(store) as Arc<dyn RlmStore>,
        template: proof_rlm::VmTemplate::from_env(),
        experiments: proof_rlm::ExperimentPolicy::default(),
        owner: Arc::new(proof_rlm::StaticOwnerHook(
            proof_rlm::OwnerDecision::Approve,
        )),
        keys: Arc::new(SealKeys),
        spend_cap_usd: None,
        skip_baseline: false,
    }
}

/// A key probe for a path that never asks for a key: sealing is not a
/// provisioning step, so nothing here may reach the owner's key file.
struct SealKeys;

impl proof_rlm::OwnerKeysProbe for SealKeys {
    fn owner_keys_present(&self) -> Result<(), proof_rlm::HookError> {
        Err(proof_rlm::HookError::Failed(
            "`topic seal` provisions nothing and asks for no owner key".into(),
        ))
    }
}

/// The measurement an `open` document must seal, built from the RLM's own
/// stored run.
///
/// `custom_value` is the measured primary: for the custom family that *is*
/// the metric. The NLL fields are zero because this family does not measure
/// them, and [`BaselineMeasurement::verify`] checks the split **count**
/// against the topic's holdout shape, so the vector has the right shape
/// without inventing numbers that would then be signed. The eval image digest
/// is the pin's, so the seal binds to the image the run was made under.
fn seal_measurement(
    pin: &ProofPin,
    document: &TopicDocument,
    measured: &BaselineRow,
) -> BaselineMeasurement {
    BaselineMeasurement {
        eval_image_digest: pin.eval_image_digest.clone(),
        topic_id: document.id.clone(),
        holdout_commitment: document.holdout_commitment.clone(),
        holdout_nll: 0.0,
        split_nll: HoldoutSplit::SCORED
            .iter()
            .map(|s| (s.as_str().to_owned(), 0.0))
            .collect(),
        tokens_per_sec: None,
        step_latency_ms: None,
        custom_value: Some(measured.primary_value),
    }
}

/// Resolve an alias and read the topic's newest document, with its version.
async fn resolve_topic(
    store: &PgRlmStore,
    topic_id: &str,
) -> Result<(String, u32, TopicDocument), OpsError> {
    let resolved = store
        .resolve_alias(topic_id)
        .await
        .map_err(|e| OpsError::error(format!("resolve {topic_id}: {e}")))?;
    let canonical = resolved.as_deref().unwrap_or(topic_id);
    let row = store
        .latest_topic(canonical)
        .await
        .map_err(|e| OpsError::error(format!("{canonical}: {e}")))?;
    let Some((version, document)) = row else {
        return Err(OpsError::error(format!(
            "no installed topic {topic_id:?}{}. Use `proof-admin topic list` to see the exact \
             ids.",
            alias_note(topic_id, canonical)
        )));
    };
    Ok((canonical.to_owned(), version, document))
}

fn alias_note(topic_id: &str, canonical: &str) -> String {
    if topic_id == canonical {
        String::new()
    } else {
        format!(" (alias of {canonical:?})")
    }
}

/// The lifecycle word, matching the wire spelling the document uses.
fn status_word(status: TopicStatus) -> &'static str {
    match status {
        TopicStatus::Draft => "draft",
        TopicStatus::Open => "open",
        TopicStatus::Closed => "closed",
    }
}

/// What the operator does with the commitment `topic baseline` printed.
fn next_seal_steps(topic_id: &str, commitment: &str) -> String {
    format!(
        "1. Put that commitment into the draft's `baseline.metrics_commitment`, set `status: \
         open`, and sign it (the `proof` key stays with you; `xtask proof-topic` signs a draft).\n\
         2. Seal it and publish:\n     proof-admin topic seal {topic_id} --document <open.json> \
         --publish --admin-url <master-or-gateway> --admin-token-file <file>\n\
         3. Confirm the host is scorable: `ctx proof status` reports `can_score`, or read \
         `GET /v1/status` (commitment {commitment})."
    )
}

/// Where a topic is in its RLM lifecycle, and what it is waiting on.
///
/// `topic install --drive-rlm` prints one line and then **nothing** until the
/// whole run returns: provisioning a VM, the RLM's `propose_rules` job, and a
/// paid baseline can legitimately take hours, and a run that is working is
/// indistinguishable from one that is stuck if the only observable is "no
/// output yet". The durable progress is the lifecycle journal, so this is the
/// read that makes a long run legible — and, when a run dies, the last
/// transition is what says how far it got.
#[derive(Debug, Clone, PartialEq)]
pub struct LifecycleReport {
    /// Canonical topic slug (the alias resolved).
    pub topic_id: String,
    /// The state the newest transition left the topic in.
    pub state: String,
    /// Rule version in force, when the RLM has written one.
    pub rules_version: Option<u32>,
    /// Whether that version is RLM-authored (`proof_rule_version.source`).
    pub rules_source: Option<String>,
    /// Whether a baseline has been measured (and under which version).
    pub baseline_rules_version: Option<u32>,
    /// Every transition, oldest first: `from -> to (event)`.
    pub history: Vec<String>,
}

impl LifecycleReport {
    /// What the operator should do next, read off the state.
    #[must_use]
    pub fn next_steps(&self) -> String {
        match self.state.as_str() {
            "draft" => "Nothing has run yet. Drive the RLM: `proof-admin topic install \
                        --bundle <bundle> --env <target> --drive-rlm --owner-approved`."
                .to_owned(),
            "owner_presend" | "awaiting_owner_keys" => {
                "The lifecycle is waiting on the owner (approval, then the owner key file). \
                 `--drive-rlm` needs `--owner-approved` and \
                 `PROOF_RLM_OWNER_INFERENCE_KEY_FILE` present."
                    .to_owned()
            }
            "provisioning" => "The VM is being created. A `--drive-rlm` run is in flight if the \
                               CLI is still attached; if it is not, this state is where it \
                               stopped — re-run the same command to resume."
                .to_owned(),
            "baselining" => {
                // A measured baseline is only sealable while the rules it was
                // measured under are still the ones in force: a vector that
                // moved since would seal a bar nobody is scored against, and
                // `mark_sealed` refuses it. Saying "seal it" from the mere
                // existence of a row would send the operator into that refusal.
                //
                // The condition mirrors `baseline_still_in_force` exactly —
                // the version in force must *equal* the measured one — so a
                // topic with no rule row at all is stale too, not sealable.
                let sealable = self.baseline_rules_version.is_some()
                    && self.baseline_rules_version == self.rules_version;
                if self.baseline_rules_version.is_some() && !sealable {
                    format!(
                        "A baseline is measured, but under rule version {} — version {} is in \
                         force now, so sealing it would publish a bar measured under rules nobody \
                         scores with. Re-run the baseline under the rules in force \
                         (`proof-admin topic install --bundle <bundle> --env <target> \
                         --drive-rlm --owner-approved`), then seal that number.",
                        self.baseline_rules_version
                            .map_or_else(|| "none".to_owned(), |v| v.to_string()),
                        self.rules_version
                            .map_or_else(|| "none".to_owned(), |v| v.to_string())
                    )
                } else if sealable {
                    "A baseline is measured. Seal it: `proof-admin topic baseline \
                     <topic>` then `topic seal … --publish`."
                        .to_owned()
                } else {
                    "No baseline yet. The RLM's `propose_rules` → `baseline` jobs run here; a \
                     paid baseline can take hours. If no CLI is attached, the run stopped — \
                     re-run `topic install --drive-rlm --owner-approved` to resume from this \
                     state."
                        .to_owned()
                }
            }
            "open" => "The topic is sealed and open in the registry. A seal without \
                       `--publish` leaves the host serving the **previous** document, so \
                       miners cannot reach it yet: confirm the open version is live \
                       (`proof-admin topic show <id>` reports the published document) and \
                       that the host is scorable (`can_score` on `GET /v1/status`) before \
                       treating it as submitable. If it is not published, re-run \
                       `topic seal … --publish` (the seal is already recorded; the same \
                       document publishes as-is)."
                .to_owned(),
            other => format!("State {other:?} is not one this command gives advice for."),
        }
    }
}

/// `topic lifecycle`: read the journal and the provenance, and say what is next.
///
/// Read-only: it never writes, never moves the lifecycle, and never spends.
///
/// # Errors
///
/// [`OpsError::error`] when the topic is unknown or the store cannot be read.
pub async fn lifecycle(pool: &sqlx::PgPool, topic_id: &str) -> Result<LifecycleReport, OpsError> {
    let store = PgRlmStore::new(pool.clone());
    let (canonical, _, _) = resolve_topic(&store, topic_id).await?;
    let lc = store
        .lifecycle(&canonical)
        .await
        .map_err(|e| OpsError::error(format!("{canonical} lifecycle: {e}")))?
        .ok_or_else(|| {
            OpsError::error(format!(
                "topic {canonical:?} has no lifecycle rows: nothing has driven it yet. \
                 `proof-admin topic install --drive-rlm --owner-approved` writes the first one."
            ))
        })?;
    let rules_version = store
        .current_rules(&canonical)
        .await
        .map_err(|e| OpsError::error(format!("{canonical} rules: {e}")))?
        .map(|r| r.version);
    let rules_source = store
        .current_rules_source(&canonical)
        .await
        .map_err(|e| OpsError::error(format!("{canonical} rule provenance: {e}")))?
        .map(|s| format!("{s:?}").to_lowercase());
    let baseline_rules_version = store
        .baseline(&canonical)
        .await
        .map_err(|e| OpsError::error(format!("{canonical} baseline: {e}")))?
        .map(|b| b.rules_version);
    let history = lc
        .history
        .iter()
        .map(|t| {
            format!(
                "{} -> {} ({})",
                t.from.as_str(),
                t.to.as_str(),
                t.event.as_str()
            )
        })
        .collect();
    Ok(LifecycleReport {
        topic_id: canonical,
        state: lc.state.as_str().to_owned(),
        rules_version,
        rules_source,
        baseline_rules_version,
        history,
    })
}

/// What to do once the topic is open.
fn after_seal(topic_id: &str, published: bool) -> String {
    if published {
        format!(
            "The topic is open and published, so miners can submit to it. Confirm the host \
             reports it scorable:\n  GET /v1/status → `can_score`, `open_topics` contains \
             {topic_id:?}\n  ctx proof status (or ctx proof topics) from a miner host"
        )
    } else {
        format!(
            "The topic is open in the registry, but the published document is still the old one \
             — re-run with --publish (the seal is already recorded; the same document publishes \
             as-is):\n  proof-admin topic seal {topic_id} --document <open.json> --publish \
             --admin-url <master-or-gateway> --admin-token-file <file>"
        )
    }
}

/// Turn a `mark_sealed` refusal into an operator instruction.
fn seal_failure(err: &proof_topic_setup::SetupError, topic_id: &str) -> String {
    use proof_topic_setup::SetupError;
    let guidance = match err {
        SetupError::NotOpen(_) => {
            "The document is not `status: open`. Sealing opens a topic; a draft is not one."
        }
        SetupError::Topic(e) => {
            return format!(
                "the document was refused: {e}\n  Nothing moved: the topic is still at its \
                 previous state and the open version was not stored.\n  What the open document \
                 needs: a registered custom id for this host (PROOF_VM_RUNNER_CUSTOM_IDS), a \
                 sealed baseline (`script_sha256` + `metrics_commitment`), and floors that only \
                 tighten the pin. Fix the draft, re-sign, and re-run."
            );
        }
        SetupError::Seal(e) => {
            return format!(
                "the seal does not bind: {e}\n  Nothing moved. The usual cause is a \
                 `metrics_commitment` that is not the one `proof-admin topic baseline` printed \
                 for this topic (it is over the measured vector, not over the file), or a \
                 `custom_value` the RLM never measured. Re-read the measurement and re-sign."
            );
        }
        SetupError::BaselineStale {
            topic_id,
            measured,
            in_force,
        } => {
            return format!(
                "the measured baseline is stale: it was taken under rule version {measured}, but \
                 version {} is in force.\n  A baseline is a measurement **against a rule \
                 version**: sealing this one would publish a bar measured under rules nobody \
                 scores with, so miners would be judged by the newer checklist while the number \
                 they must beat came from the older one. Nothing moved.\n  Re-run the baseline \
                 under the rules in force:\n    proof-admin topic install --bundle <bundle> \
                 --env <target> --drive-rlm --owner-approved\n  Then read \
                 `proof-admin topic baseline {topic_id}` again and seal the new commitment.",
                in_force.map_or_else(|| "none".to_owned(), |v| v.to_string())
            );
        }
        SetupError::DegenerateBar { topic_id, primary } => {
            return format!(
                "the measured baseline is a degenerate bar ({primary}) for topic {topic_id:?}, so \
                 it will not be sealed.\n  This family scores a **relative** win \
                 (`challenger >= bar * (1 + epsilon_rel)`), which has no solution when the bar is \
                 zero: the topic would be open, scorable, and impossible for every miner to pass. \
                 A zero bar is a real measurement — a reference run that solved nothing, which is \
                 what an all-zero Harbor baseline is — not a defect in this command.\n  Nothing \
                 was changed and **nothing was auto-resealed**: the stored measurement is exactly \
                 what the RLM wrote. To get a sealable baseline, re-run it against a reference \
                 that can actually score, or fix the task selection so the reference run measures \
                 something:\n    1. `proof-admin topic baseline {topic_id}` shows the measurement \
                 the RLM left, with the rules version it was taken under.\n    2. Re-drive the \
                 RLM with a reference that scores: `proof-admin topic install --bundle <bundle> \
                 --env <target> --drive-rlm --owner-approved`.\n    3. Re-read \
                 `proof-admin topic baseline {topic_id}` and seal the new commitment."
            );
        }
        SetupError::State(proof_rlm::StateError::Illegal { from, .. }) => {
            return format!(
                "the topic's lifecycle is at {from:?}, not `baselining`, so there is no \
                 measured baseline to seal against. Either the install never drove the RLM \
                 (`--drive-rlm`) or it was sealed already. `proof-admin topic install-log \
                 --topic {topic_id}` shows the install; `proof-admin topic show {topic_id}` \
                 shows the current state."
            );
        }
        _ => "Nothing moved; the topic is still at its previous state.",
    };
    format!("the seal was refused: {err}\n  {guidance}")
}

/// Turn a publish refusal into an operator instruction.
fn publish_failure(why: &str, topic_id: &str) -> String {
    format!(
        "the seal is recorded and the topic is **open**, but the publish failed: {why}\n  The \
         published document is still the previous one, so miners cannot reach the open topic \
         yet. Nothing is wrong with the seal: re-run the same command with --publish (the seal \
         is idempotent from `open` — it publishes the document you pass) once the admin URL or \
         bearer is fixed.\n  If the route refused an `open` document because the install is not \
         `applied`, finish the install first: `proof-admin topic install-log --topic \
         {topic_id}`."
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use proof_rlm::fixtures;
    use proof_task::MetricFamily;

    /// The measured baseline the RLM leaves in `proof_baseline_measurement`.
    fn measured(primary: f64) -> BaselineRow {
        BaselineRow {
            topic_id: "tb4".into(),
            rules_version: 3,
            primary_value: primary,
            report: proof_rlm::CustomRunReport {
                schema_version: proof_rlm::RUN_REPORT_SCHEMA,
                topic_id: "tb4".into(),
                custom_id: "tb4-metric".into(),
                submission_digest: "11".repeat(32),
                artifact_digest: "22".repeat(32),
                rules_version: 3,
                primary_value: primary,
                claim_holds: true,
                sandboxed: true,
                flops_used: None,
                evidence: std::collections::BTreeMap::new(),
                results: None,
            },
        }
    }

    fn document() -> TopicDocument {
        let mut doc = TopicDocument {
            id: "tb4".into(),
            status: TopicStatus::Open,
            ..TopicDocument::default()
        };
        doc.metric.family = MetricFamily::Custom;
        doc.metric.custom_id = "tb4-metric".into();
        doc.holdout_commitment = "cd".repeat(32);
        doc.baseline.script_sha256 = "ee".repeat(32);
        doc.baseline.metrics_commitment.clear();
        doc
    }

    /// The two halves agree: the commitment `topic baseline` prints is the one
    /// `mark_sealed` accepts when the document carries it, and a document that
    /// seals anything else is refused. This is the wiring the live ceremony
    /// depends on, checked without a database or a signature.
    #[test]
    fn the_commitment_we_print_is_the_one_mark_sealed_verifies() {
        let pin = fixtures::pin();
        let row = measured(0.42);
        let sealed = seal_measurement(&pin, &document(), &row);
        assert_eq!(sealed.custom_value, Some(0.42));
        assert_eq!(
            sealed.eval_image_digest, pin.eval_image_digest,
            "the seal binds to the pinned eval image"
        );
        assert_eq!(
            sealed.split_nll.len(),
            HoldoutSplit::SCORED.len(),
            "the vector has the topic's holdout shape"
        );

        // The document that carries what we printed verifies.
        let mut open = document();
        open.baseline.metrics_commitment = sealed.commitment();
        sealed
            .verify(&pin, &open)
            .expect("the printed commitment is the one the runtime accepts");

        // A document sealing a different value is refused — the seal is over
        // the measured vector, not over the file.
        let mut wrong = open.clone();
        wrong.baseline.metrics_commitment = "00".repeat(32);
        assert!(sealed.verify(&pin, &wrong).is_err());

        // And a measurement that is not the measured primary is refused even
        // when the document agrees with itself.
        let other = seal_measurement(&pin, &document(), &measured(0.99));
        let mut open_other = document();
        open_other.baseline.metrics_commitment = other.commitment();
        assert!(other.verify(&pin, &open_other).is_ok());
        assert!(
            other.verify(&pin, &open).is_err(),
            "0.99 cannot verify against a document sealing 0.42"
        );
    }

    /// `topic baseline` is the read **before** the operator signs an `open`
    /// document, so it has to say when the measurement cannot be sealed.
    ///
    /// Warning only at seal time is correct but late: the document would
    /// already carry a number the seal refuses, and the operator would have
    /// signed and published a topic that can never open. The flag is the same
    /// predicate `mark_sealed` refuses on, so the two cannot disagree.
    #[test]
    fn a_degenerate_measurement_is_flagged_before_the_document_is_signed() {
        use proof_task::MetricSpec;
        let custom = |primary_value: f64| {
            let mut doc = document();
            doc.metric = MetricSpec {
                family: MetricFamily::Custom,
                primary: "custom_value".into(),
                custom_id: "placeholder_metric".into(),
                epsilon_rel: 0.05,
                ..doc.metric.clone()
            };
            baseline_is_degenerate(&doc, primary_value)
        };
        assert!(
            custom(0.0),
            "a zero primary on a relative family is flagged"
        );
        assert!(
            !custom(0.42),
            "a real measurement is not flagged: the guard narrows nothing else"
        );

        // `nll` compares absolutely, so a zero bar there is a hard but
        // meaningful target — the flag must not reach across families.
        let mut nll = document();
        nll.metric.family = MetricFamily::Nll;
        assert!(
            !baseline_is_degenerate(&nll, 0.0),
            "the absolute family is never degenerate"
        );
    }

    /// `open` is a registry state, not a promise that miners can submit.    ///
    /// A seal without `--publish` reaches `open` while the host still serves
    /// the previous document, so the lifecycle advice must not tell an operator
    /// the topic is reachable. It has to say the seal is recorded, that the
    /// publish is the step that makes it live, and how to retry it.
    #[test]
    fn the_open_advice_does_not_claim_the_topic_is_published() {
        let report = LifecycleReport {
            topic_id: "tb4".into(),
            state: "open".into(),
            rules_version: Some(2),
            rules_source: Some("rlm".into()),
            baseline_rules_version: Some(2),
            history: Vec::new(),
        };
        let advice = report.next_steps();
        assert!(
            advice.contains("--publish"),
            "the advice must name the publish step: {advice}"
        );
        assert!(
            advice.to_lowercase().contains("previous"),
            "the advice must say the previous document is still served: {advice}"
        );
        assert!(
            advice.contains("can_score"),
            "the advice must say to confirm the host is scorable: {advice}"
        );
        assert!(
            !advice.contains("Miners can submit;"),
            "the advice must not promise submissions on `open` alone: {advice}"
        );
    }

    /// The state-specific advice names the state it is talking about.
    #[test]
    fn the_lifecycle_advice_matches_the_state() {
        let make = |state: &str| LifecycleReport {
            topic_id: "tb4".into(),
            state: state.into(),
            rules_version: None,
            rules_source: None,
            baseline_rules_version: None,
            history: Vec::new(),
        };
        // The in-flight state is the one a long `--drive-rlm` sits in, so the
        // advice has to say a run may be working rather than lost.
        assert!(make("provisioning")
            .next_steps()
            .contains("VM is being created"));
        // `baselining` with no baseline is the paid job not having landed.
        let baselining = make("baselining").next_steps();
        assert!(
            baselining.contains("No baseline yet"),
            "an unmeasured baseline must say so: {baselining}"
        );
        // …and with one, the next step is the seal.
        let mut measured = make("baselining");
        measured.baseline_rules_version = Some(2);
        measured.rules_version = Some(2);
        assert!(measured.next_steps().contains("Seal it"));
        // A topic nothing has driven says so rather than inventing advice.
        assert!(make("draft").next_steps().contains("Nothing has run yet"));
    }

    /// A baseline measured under a superseded vector is not sealable, so the
    /// `baselining` advice must not send the operator into that refusal.
    ///
    /// The baseline is a measurement **against a rule version**: sealing a
    /// stale one would publish a bar measured under rules nobody scores with,
    /// while miners are judged by the vector in force.
    #[test]
    fn the_baselining_advice_refuses_to_recommend_sealing_a_stale_baseline() {
        let make = |baseline: Option<u32>, current: Option<u32>| LifecycleReport {
            topic_id: "tb4".into(),
            state: "baselining".into(),
            rules_version: current,
            rules_source: Some("rlm".into()),
            baseline_rules_version: baseline,
            history: Vec::new(),
        };

        let stale = make(Some(1), Some(2)).next_steps();
        assert!(
            !stale.contains("Seal it"),
            "a stale baseline must not be recommended for sealing: {stale}"
        );
        assert!(
            stale.contains("version 1") && stale.contains("version 2"),
            "the advice must name both versions: {stale}"
        );
        assert!(
            stale.contains("--drive-rlm"),
            "the advice must name the way to re-measure: {stale}"
        );

        // The version in force matching the measurement is still a seal.
        assert!(make(Some(2), Some(2)).next_steps().contains("Seal it"));
        // No rule row at all is a different problem, not a stale baseline:
        // the message must not claim a version is in force when none is.
        let no_rules = make(Some(2), None).next_steps();
        assert!(
            !no_rules.contains("Seal it"),
            "a baseline with no rules in force is not sealable: {no_rules}"
        );
    }
}
