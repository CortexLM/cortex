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
//! proof-admin topic baseline tb4
//!
//! # 2. Sign the open document carrying that commitment, then:
//! proof-admin topic seal tb4 --document open.json --publish \
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
    /// New document version the seal stored.
    pub document_version: u32,
    /// The primary the RLM measured and the document sealed.
    pub primary_value: f64,
    /// The commitment the document carries.
    pub metrics_commitment: String,
    /// Whether the open document was published through the admin route.
    pub published: bool,
}

impl SealOutcome {
    /// What to do next, for the operator.
    #[must_use]
    pub fn after(&self) -> String {
        after_seal(&self.topic_id, self.published)
    }
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
    // The one call that decides: the same `mark_sealed` the runtime's own
    // tests drive, so a document this command accepts is one the scoring path
    // would accept.
    let registered: Vec<&str> = args.registered_custom.iter().map(String::as_str).collect();
    let setup = seal_setup(store);
    setup
        .mark_sealed(&document, args.pin, &registered, &sealed)
        .await
        .map_err(|e| OpsError::error(seal_failure(&e, &canonical)))?;
    let commitment = sealed.commitment();

    if args.publish {
        let admin = PublishTarget::resolve(args.admin_url, args.admin_token_file)?;
        admin
            .publish(&document)
            .await
            .map_err(|e| OpsError::error(publish_failure(&e, &canonical)))?;
    }
    Ok(SealOutcome {
        topic_id: canonical,
        document_version: version + 1,
        primary_value: measured.primary_value,
        metrics_commitment: commitment,
        published: args.publish,
    })
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
}
