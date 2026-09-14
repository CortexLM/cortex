//! Proof **topic install bundle**: the operator procedure that publishes one
//! signed topic.
//!
//! This crate deliberately does **not** define a second topic registry. A
//! Proof topic already has one home: the operator-signed [`TopicDocument`]
//! (`proof-task`), published through `POST /v1/admin/proof/topics` and
//! persisted in `proof_topic_version` (migration `0020`). The bindings a topic
//! needs are already signed topic data too — `constraints.params` carries the
//! in-guest runner and its pinned pack digest (`proof-experiment`), and the
//! image pins are operator env.
//!
//! What was missing is the *procedure*: which signed document, which install
//! target, and which host env must agree with it before the topic can run.
//! That is this bundle. It **references** the document and **cross-checks**
//! the host expectations against it; it never restates a binding in a second
//! place that could drift.
//!
//! Three rules carry the fail-closed posture:
//!
//! - **Unknown keys are refused.** A field this build does not understand is a
//!   step nothing performs, so `deny_unknown_fields` rejects it at parse.
//! - **A digest is never invented.** Every expected pin is
//!   `sha256:<64 lowercase hex>`, checked against the same shape the host env
//!   uses.
//! - **The document wins.** A host expectation that disagrees with the signed
//!   document is a reject, not a silent override: the signature is what the
//!   scoring path trusts, so an operator env that says otherwise would mean
//!   the topic runs something other than what was signed.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::doc_markdown
)]

use std::fmt;
use std::str::FromStr;

use proof_experiment::{ExperimentBinding, ExperimentError};
use proof_task::{MetricFamily, TopicDocument, TopicError, TopicStatus};
use serde::{Deserialize, Serialize};

/// Only accepted `schema_version`.
pub const BUNDLE_SCHEMA_VERSION: u32 = 1;

/// Longest legal `display_name`.
pub const MAX_DISPLAY_NAME_LEN: usize = 128;

/// Every key the bundle schema accepts, sorted. The schema is this Rust type
/// (`deny_unknown_fields`), not a second document that could drift from it:
/// this list is what a test pins, so adding or removing a key is a deliberate
/// edit here rather than a silent widening of what an operator may write.
pub const BUNDLE_KEYS: [&str; 5] = [
    "display_name",
    "environment",
    "host",
    "schema_version",
    "topic",
];

/// Keys with no `serde` default: a bundle that omits one is a parse error
/// naming the field, never an empty value that fails later.
pub const REQUIRED_BUNDLE_KEYS: [&str; 5] = [
    "display_name",
    "environment",
    "host",
    "schema_version",
    "topic",
];

/// Keys of the `host` block, sorted.
pub const HOST_KEYS: [&str; 5] = [
    "custom_ids_entry",
    "experiment_image_digest",
    "pack_digest",
    "pack_dir",
    "rlm_image_digest",
];

/// Install targets, in the order the CLI offers them.
pub const INSTALL_ENVIRONMENTS: [&str; 2] = ["staging", "metal"];

/// Prefix of every digest.
pub const DIGEST_PREFIX: &str = "sha256:";

/// The existing admin publish route this bundle prepares a call for.
///
/// Not a new route: `proof-http` already serves it, and the CLI's `validate`
/// runs the same acceptance checks that route runs before it writes.
pub const PUBLISH_ROUTE: &str = "POST /v1/admin/proof/topics";

/// The publish path as it appears in a printed `curl` line.
pub const PUBLISH_PATH: &str = "/challenge/proof/v1/admin/proof/topics";

/// Operator env naming the custom ids the host will score.
pub const ENV_CUSTOM_IDS: &str = "PROOF_VM_RUNNER_CUSTOM_IDS";

/// Operator env pinning the RLM VM image.
pub const ENV_RLM_IMAGE: &str = "PROOF_RLM_VM_IMAGE_DIGEST";

/// Operator env pinning the experiment guest image.
pub const ENV_EXPERIMENT_IMAGE: &str = "PROOF_EXPERIMENT_VM_IMAGE_DIGEST";

/// Operator env holding the directory of staged experiment packs.
pub const ENV_PACK_DIR: &str = "PROOF_VM_AGENT_EXPERIMENT_PACK_DIR";

/// Where a topic may be installed.
///
/// The install target is operator state, not topic data: the same bundle is
/// installed to staging first and to metal later, and `bins/proof-admin`
/// refuses when the flag and the bundle disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallEnvironment {
    /// Staging host. Nothing here scores live.
    Staging,
    /// Live metal.
    Metal,
}

impl InstallEnvironment {
    /// Wire word (`staging` / `metal`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Staging => "staging",
            Self::Metal => "metal",
        }
    }
}

impl fmt::Display for InstallEnvironment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for InstallEnvironment {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "staging" => Ok(Self::Staging),
            "metal" => Ok(Self::Metal),
            other => Err(format!(
                "{other:?} is not an install target ({})",
                INSTALL_ENVIRONMENTS.join(" | ")
            )),
        }
    }
}

/// Why a bundle was refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum BundleError {
    /// The body was not JSON, or carried a key this build does not know.
    #[error("parse topic install bundle: {0}")]
    Parse(String),
    /// Schema version drift.
    #[error("schema_version {got}, this build reads {want}")]
    WrongSchema {
        /// What the bundle said.
        got: u32,
        /// What this build reads.
        want: u32,
    },
    /// `display_name` is empty or oversized.
    #[error("display_name must be 1..={MAX_DISPLAY_NAME_LEN} chars")]
    BadDisplayName,
    /// A host expectation is not `sha256:<64 lowercase hex>`.
    #[error("host.{field} {got:?} is not {DIGEST_PREFIX}<64 lowercase hex>")]
    BadDigest {
        /// Which expectation (`rlm_image_digest`, `experiment_image_digest`, `pack_digest`).
        field: &'static str,
        /// What the bundle said.
        got: String,
    },
    /// The document selects an in-guest runner but nothing pins its pack.
    #[error(
        "the signed document selects in-guest runner {runner_id:?}, so host.pack_digest is \
         required and must equal the document's constraints.params.experiment_pack_digest \
         (sha256:<64 hex>; never invented)"
    )]
    RunnerWithoutPack {
        /// The runner the document selected.
        runner_id: String,
    },
    /// A pack is pinned that nothing runs.
    #[error(
        "host.pack_digest is set but the signed document selects no in-guest runner: a pack no \
         runner reads is dead weight, and this bundle would stage it anyway"
    )]
    PackWithoutRunner,
    /// A host expectation disagrees with the signed document.
    #[error("host.{field} {got:?} contradicts the signed document, which says {document:?}")]
    HostContradictsDocument {
        /// Which expectation.
        field: &'static str,
        /// What the bundle said.
        got: String,
        /// What the document says.
        document: String,
    },
    /// `custom_ids_entry` names no custom id while the document is custom.
    #[error(
        "{ENV_CUSTOM_IDS} does not register this topic's metric.custom_id {custom_id:?}; an \
         open custom topic whose id is not registered cannot score (503)"
    )]
    CustomIdNotRegistered {
        /// The topic's `metric.custom_id`.
        custom_id: String,
    },
    /// `pack_dir` is not an absolute path with no traversal.
    #[error("host.pack_dir {0:?} must be an absolute path with no `..` segment")]
    BadPackDir(String),
    /// The document itself was refused by the shared topic checks.
    #[error("topic document: {0}")]
    Topic(#[from] TopicError),
    /// The document's `constraints.params` are not a usable runner binding.
    #[error("topic binding: {0}")]
    Binding(#[from] ExperimentError),
    /// The canonical form could not be built.
    #[error("canonicalize bundle: {0}")]
    Canonicalize(String),
    /// The `--env` flag and the bundle's `environment` disagree.
    #[error("bundle declares environment {bundle}, but --env {requested} was requested")]
    EnvironmentMismatch {
        /// What the bundle declared.
        bundle: InstallEnvironment,
        /// What the operator asked for.
        requested: InstallEnvironment,
    },
}

/// The operator env that must agree with the signed document.
///
/// Every field is optional: an operator writes only what the topic needs. What
/// is present is checked against the document, never used to override it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct HostExpectations {
    /// `PROOF_RLM_VM_IMAGE_DIGEST` the host must pin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rlm_image_digest: Option<String>,
    /// Experiment guest image the host must pin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experiment_image_digest: Option<String>,
    /// The experiment pack the KVM host must hold, staged under
    /// `PROOF_VM_AGENT_EXPERIMENT_PACK_DIR`. Must equal the document's
    /// `constraints.params.experiment_pack_digest` when the document selects
    /// an in-guest runner.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pack_digest: Option<String>,
    /// The **directory** the pack tar is staged in on the KVM host, i.e. the
    /// value of `PROOF_VM_AGENT_EXPERIMENT_PACK_DIR`.
    ///
    /// A path, never a digest: the variable names a directory, and the host
    /// re-hashes the tar it finds there against the document's pin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pack_dir: Option<String>,
    /// Comma-separated value the host will set as `PROOF_VM_RUNNER_CUSTOM_IDS`.
    /// An open custom topic scores only when this registers its
    /// `metric.custom_id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_ids_entry: Option<String>,
}

/// One topic install bundle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopicInstallBundle {
    /// Must equal [`BUNDLE_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Install target this bundle was written for.
    pub environment: InstallEnvironment,
    /// Human label for operator output. Not topic data: the scoring contract
    /// is the signed document below.
    pub display_name: String,
    /// The signed topic document, verbatim.
    pub topic: TopicDocument,
    /// Operator env this install needs.
    pub host: HostExpectations,
}

/// One operator env line the SOP asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostEnvVar {
    /// Variable name.
    pub name: String,
    /// Value the host must set.
    pub value: String,
    /// Why, in operator English.
    pub why: String,
}

/// The resolved install: what a `--dry-run` prints and what an operator runs.
///
/// Everything here is derived from the signed document or from the bundle's
/// own target. Nothing is stored: the registry remains `proof_topic_version`,
/// and this plan is a procedure, not a row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TopicInstallPlan {
    /// Topic slug (the document's `id`).
    pub topic_id: String,
    /// Human label.
    pub display_name: String,
    /// Install target.
    pub environment: InstallEnvironment,
    /// Lifecycle the signed document declares.
    pub document_status: TopicStatus,
    /// Metric family the document declares.
    pub metric_family: MetricFamily,
    /// `metric.custom_id` (empty on non-custom families).
    pub custom_id: String,
    /// In-guest runner the document selects, when it selects one.
    pub runner_id: Option<String>,
    /// Experiment pack digest the document pins, when it pins one.
    pub pack_digest: Option<String>,
    /// The existing admin route that publishes this document.
    pub publish_route: String,
    /// Operator env lines this install needs, in the order to set them.
    pub host_env: Vec<HostEnvVar>,
    /// Where the pack is staged on the KVM host, when a pack is pinned.
    pub pack_dir_env: Option<String>,
    /// `sha256:<hex>` over the canonical bundle.
    pub bundle_digest: String,
}

fn is_digest(s: &str) -> bool {
    s.strip_prefix(DIGEST_PREFIX).is_some_and(is_lower_hex64)
}

/// Exactly 64 **lowercase** hex characters, with no surrounding whitespace.
///
/// Deliberately stricter than `proof_canon::is_hex64`, which trims and accepts
/// uppercase: the host env this mirrors is compared verbatim, so accepting
/// `sha256:AB…` or `sha256: ab… ` would let a bundle validate and then
/// disagree with the pin actually staged. One spelling of a digest.
fn is_lower_hex64(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Split a `PROOF_VM_RUNNER_CUSTOM_IDS` value into ids.
///
/// Comma- or whitespace-separated, trimmed, empties dropped — the same shape
/// `proof-challenge` parses from that variable.
#[must_use]
pub fn parse_custom_ids(raw: &str) -> Vec<String> {
    raw.split([',', ' ', '\t', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

impl TopicInstallBundle {
    /// Parse a bundle body (JSON only).
    ///
    /// Unknown keys are refused here: a step this build cannot name is a step
    /// nothing performs.
    pub fn from_json(body: &str) -> Result<Self, BundleError> {
        serde_json::from_str(body).map_err(|e| BundleError::Parse(e.to_string()))
    }

    /// Shape checks that need no pin: schema, label, digest spellings, and the
    /// cross-checks between the host block and the signed document.
    ///
    /// The document's own floors, signature, and seal are **not** checked
    /// here — that is [`Self::accept`], which runs the same acceptance the
    /// admin publish route runs.
    pub fn validate_shape(&self) -> Result<(), BundleError> {
        if self.schema_version != BUNDLE_SCHEMA_VERSION {
            return Err(BundleError::WrongSchema {
                got: self.schema_version,
                want: BUNDLE_SCHEMA_VERSION,
            });
        }
        let name = self.display_name.trim();
        if name.is_empty() || name.chars().count() > MAX_DISPLAY_NAME_LEN {
            return Err(BundleError::BadDisplayName);
        }
        for (field, value) in [
            ("rlm_image_digest", self.host.rlm_image_digest.as_deref()),
            (
                "experiment_image_digest",
                self.host.experiment_image_digest.as_deref(),
            ),
            ("pack_digest", self.host.pack_digest.as_deref()),
        ] {
            if let Some(v) = value {
                if !is_digest(v) {
                    return Err(BundleError::BadDigest {
                        field,
                        got: v.to_owned(),
                    });
                }
            }
        }
        self.check_pack_dir()?;
        self.cross_check_host()
    }

    /// Whether `pack_dir` is a usable directory value.
    fn check_pack_dir(&self) -> Result<(), BundleError> {
        let Some(dir) = self.host.pack_dir.as_deref() else {
            return Ok(());
        };
        let d = dir.trim();
        let ok = d.starts_with('/')
            && d.len() <= 512
            && !d.chars().any(char::is_control)
            && !d.split('/').any(|seg| seg == "..");
        if ok {
            Ok(())
        } else {
            Err(BundleError::BadPackDir(dir.to_owned()))
        }
    }

    /// The in-guest runner binding the **signed document** carries, if any.
    pub fn binding(&self) -> Result<Option<ExperimentBinding>, BundleError> {
        Ok(ExperimentBinding::from_params(
            &self.topic.constraints.params,
        )?)
    }

    /// Cross-check the host block against the signed document.
    ///
    /// The document is authoritative. A disagreement is a reject: the
    /// signature is what the scoring path trusts, so an operator env that says
    /// otherwise would run something other than what was signed.
    fn cross_check_host(&self) -> Result<(), BundleError> {
        let binding = self.binding()?;
        match (&binding, self.host.pack_digest.as_deref()) {
            (Some(b), Some(host_digest)) => {
                if host_digest != b.pack.digest {
                    return Err(BundleError::HostContradictsDocument {
                        field: "pack_digest",
                        got: host_digest.to_owned(),
                        document: b.pack.digest.clone(),
                    });
                }
            }
            // A runner with nothing to run cannot score, so the pack travels
            // with it — the same rule the document's own params enforce.
            (Some(b), None) => {
                return Err(BundleError::RunnerWithoutPack {
                    runner_id: b.runner.clone(),
                });
            }
            (None, Some(_)) => return Err(BundleError::PackWithoutRunner),
            (None, None) => {}
        }
        // A custom topic is scored by the runner registered under its
        // `metric.custom_id`; an open one whose id is not registered answers
        // 503. When the bundle declares the host's id list, it has to contain
        // that id.
        if self.topic.metric.family == MetricFamily::Custom {
            let id = self.topic.metric.custom_id.trim();
            if let Some(raw) = self.host.custom_ids_entry.as_deref() {
                if !parse_custom_ids(raw).iter().any(|e| e == id) {
                    return Err(BundleError::CustomIdNotRegistered {
                        custom_id: id.to_owned(),
                    });
                }
            }
        }
        Ok(())
    }

    /// The custom ids this bundle's host block registers, for the shared
    /// acceptance check (an `open` custom topic needs its id registered).
    #[must_use]
    pub fn registered_custom(&self) -> Vec<String> {
        self.host
            .custom_ids_entry
            .as_deref()
            .map_or_else(Vec::new, parse_custom_ids)
    }

    /// Canonical JSON of the bundle: sorted keys, no insignificant
    /// whitespace. This is what [`Self::digest`] hashes, so two files that
    /// differ only in formatting install as the same bundle.
    pub fn canonical(&self) -> Result<String, BundleError> {
        let value =
            serde_json::to_value(self).map_err(|e| BundleError::Canonicalize(e.to_string()))?;
        Ok(proof_canon::canonical_json(&value))
    }

    /// `sha256:<64 hex>` over [`Self::canonical`].
    pub fn digest(&self) -> Result<String, BundleError> {
        use sha2::{Digest, Sha256};
        let canonical = self.canonical()?;
        let mut hasher = Sha256::new();
        hasher.update(canonical.as_bytes());
        Ok(format!("{DIGEST_PREFIX}{}", hex::encode(hasher.finalize())))
    }

    /// Resolve the install plan for `requested`, refusing a bundle whose
    /// declared `environment` is not the target being installed to.
    pub fn plan(&self, requested: InstallEnvironment) -> Result<TopicInstallPlan, BundleError> {
        self.validate_shape()?;
        if self.environment != requested {
            return Err(BundleError::EnvironmentMismatch {
                bundle: self.environment,
                requested,
            });
        }
        let binding = self.binding()?;
        let custom_id = self.topic.metric.custom_id.trim().to_owned();
        let mut host_env = Vec::new();
        if let Some(digest) = self.host.rlm_image_digest.as_deref() {
            host_env.push(HostEnvVar {
                name: ENV_RLM_IMAGE.to_owned(),
                value: digest.to_owned(),
                why: "RLM VM image the orchestrator boots for this topic".to_owned(),
            });
        }
        if let Some(digest) = self.host.experiment_image_digest.as_deref() {
            host_env.push(HostEnvVar {
                name: ENV_EXPERIMENT_IMAGE.to_owned(),
                value: digest.to_owned(),
                why: "guest image for this topic's in-guest runner jobs".to_owned(),
            });
        }
        if let Some(raw) = self.host.custom_ids_entry.as_deref() {
            host_env.push(HostEnvVar {
                name: ENV_CUSTOM_IDS.to_owned(),
                value: raw.trim().to_owned(),
                why: format!(
                    "registers {custom_id:?} so this host can score it (empty registry = 503)"
                ),
            });
        }
        // `PROOF_VM_AGENT_EXPERIMENT_PACK_DIR` names a **directory**, so the
        // value comes from `host.pack_dir`, never from the pack digest. The
        // digest is what the host re-hashes the staged tar against, and it
        // travels in the `why` so the operator can check both.
        if let (Some(dir), Some(digest)) = (
            self.host.pack_dir.as_deref(),
            self.host.pack_digest.as_deref(),
        ) {
            host_env.push(HostEnvVar {
                name: ENV_PACK_DIR.to_owned(),
                value: dir.trim().to_owned(),
                why: format!(
                    "stage the pack tar here; the host re-hashes it and refuses a mismatch \
                     against {digest}"
                ),
            });
        }
        Ok(TopicInstallPlan {
            topic_id: self.topic.id.clone(),
            display_name: self.display_name.trim().to_owned(),
            environment: self.environment,
            document_status: self.topic.status,
            metric_family: self.topic.metric.family,
            custom_id,
            runner_id: binding.as_ref().map(|b| b.runner.clone()),
            pack_digest: binding.as_ref().map(|b| b.pack.digest.clone()),
            publish_route: PUBLISH_ROUTE.to_owned(),
            host_env,
            pack_dir_env: binding.as_ref().map(|_| ENV_PACK_DIR.to_owned()),
            bundle_digest: self.digest()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proof_task::{
        default_adamw, holdout_commitment, synthetic_holdout, MetricSpec, PayoutMode, STRATUM_SIZE,
    };

    const HEX: &str = "abababababababababababababababababababababababababababababababab";

    fn digest() -> String {
        format!("{DIGEST_PREFIX}{HEX}")
    }

    /// A custom topic that selects an in-guest runner, the shape the live
    /// `tb4` topic has.
    fn custom_topic(custom_id: &str) -> TopicDocument {
        let mut doc = TopicDocument {
            id: "tb4".into(),
            statement: "Score the pinned task pack with the pinned runner.".into(),
            payout_mode: PayoutMode::Discovery,
            metric: MetricSpec {
                family: MetricFamily::Custom,
                primary: "primary_value".into(),
                custom_id: custom_id.into(),
                epsilon_rel: 0.05,
                ..MetricSpec::default()
            },
            baseline: default_adamw(proof_task::FLOPS_BUDGET_MAX),
            holdout_commitment: holdout_commitment(&synthetic_holdout(STRATUM_SIZE, 1)),
            ..TopicDocument::default()
        };
        doc.constraints.params.insert(
            proof_experiment::PARAM_RUNNER.into(),
            "rlm_fc_in_guest_harbor".into(),
        );
        doc.constraints
            .params
            .insert(proof_experiment::PARAM_PACK_DIGEST.into(), digest());
        doc
    }

    fn tb4() -> TopicInstallBundle {
        TopicInstallBundle {
            schema_version: BUNDLE_SCHEMA_VERSION,
            environment: InstallEnvironment::Metal,
            display_name: "Terminal-Bench 4".into(),
            topic: custom_topic("tbench"),
            host: HostExpectations {
                rlm_image_digest: Some(digest()),
                experiment_image_digest: Some(digest()),
                pack_digest: Some(digest()),
                pack_dir: Some("/var/lib/proof/packs".into()),
                custom_ids_entry: Some("tbench".into()),
            },
        }
    }

    #[test]
    fn the_arch_default_bundle_plans_against_the_existing_admin_route() {
        let bundle = tb4();
        bundle.validate_shape().expect("validates");
        let plan = bundle.plan(InstallEnvironment::Metal).expect("plan");
        assert_eq!(plan.topic_id, "tb4");
        assert_eq!(plan.environment, InstallEnvironment::Metal);
        assert_eq!(plan.custom_id, "tbench");
        assert_eq!(plan.runner_id.as_deref(), Some("rlm_fc_in_guest_harbor"));
        assert_eq!(plan.pack_digest.as_deref(), Some(digest().as_str()));
        assert_eq!(
            plan.publish_route, PUBLISH_ROUTE,
            "the plan names the existing route, not a new one"
        );
        assert_eq!(plan.pack_dir_env.as_deref(), Some(ENV_PACK_DIR));
        let names: Vec<&str> = plan.host_env.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            [
                ENV_RLM_IMAGE,
                ENV_EXPERIMENT_IMAGE,
                ENV_CUSTOM_IDS,
                ENV_PACK_DIR
            ]
        );
        assert!(plan.bundle_digest.starts_with(DIGEST_PREFIX));
        assert_eq!(plan.bundle_digest.len(), DIGEST_PREFIX.len() + 64);
    }

    /// The bundle carries the signed document verbatim, so there is exactly
    /// one copy of every binding in the system.
    #[test]
    fn the_document_is_carried_verbatim_and_is_the_only_source_of_truth() {
        let mut bundle = tb4();
        bundle.topic.signature = "cd".repeat(64);
        let value = serde_json::to_value(&bundle).expect("json");
        assert_eq!(
            value["topic"]["constraints"]["params"][proof_experiment::PARAM_RUNNER],
            "rlm_fc_in_guest_harbor"
        );
        assert_eq!(
            value["topic"]["metric"]["custom_id"], "tbench",
            "the custom id is the document's, not a bundle field"
        );
        assert_eq!(
            value["topic"]["signature"],
            "cd".repeat(64),
            "the signature travels with the document"
        );
        // The bundle itself has no place to restate a binding.
        let keys: Vec<&String> = value.as_object().expect("object").keys().collect();
        for forbidden in [
            "runner_id",
            "custom_id",
            "pin_rlm",
            "n_concurrent",
            "sealed_custom_value",
        ] {
            assert!(
                !keys.iter().any(|k| k.as_str() == forbidden),
                "the bundle must not duplicate topic data: {forbidden}"
            );
        }
    }

    #[test]
    fn the_schema_key_lists_match_the_type() {
        let bundle = tb4();
        let value = serde_json::to_value(&bundle).expect("json");
        let mut keys: Vec<String> = value.as_object().expect("object").keys().cloned().collect();
        keys.sort_unstable();
        assert_eq!(keys, BUNDLE_KEYS, "the bundle key list drifted");

        let mut host_keys: Vec<String> = value["host"]
            .as_object()
            .expect("host object")
            .keys()
            .cloned()
            .collect();
        host_keys.sort_unstable();
        assert_eq!(host_keys, HOST_KEYS, "the host key list drifted");
        for key in REQUIRED_BUNDLE_KEYS {
            assert!(keys.iter().any(|k| k == key), "{key} must be required");
        }
    }

    #[test]
    fn unknown_keys_are_refused_at_parse() {
        let body = r#"{
            "schema_version": 1, "environment": "metal", "display_name": "x",
            "topic": {}, "runner_id": "rlm_fc_in_guest_harbor"
        }"#;
        let err = TopicInstallBundle::from_json(body).expect_err("unknown key");
        assert!(
            matches!(err, BundleError::Parse(ref m) if m.contains("runner_id")),
            "{err}"
        );

        let host_body = r#"{
            "schema_version": 1, "environment": "metal", "display_name": "x",
            "topic": {}, "host": {"custom_id": "tbench"}
        }"#;
        let err = TopicInstallBundle::from_json(host_body).expect_err("unknown host key");
        assert!(
            matches!(err, BundleError::Parse(ref m) if m.contains("custom_id")),
            "{err}"
        );
    }

    #[test]
    fn required_keys_are_named_when_absent() {
        for (body, missing) in [
            (
                r#"{"environment":"metal","display_name":"x","topic":{},"host":{}}"#,
                "schema_version",
            ),
            (
                r#"{"schema_version":1,"display_name":"x","topic":{},"host":{}}"#,
                "environment",
            ),
            (
                r#"{"schema_version":1,"environment":"metal","topic":{},"host":{}}"#,
                "display_name",
            ),
            (
                r#"{"schema_version":1,"environment":"metal","display_name":"x","host":{}}"#,
                "topic",
            ),
            (
                r#"{"schema_version":1,"environment":"metal","display_name":"x","topic":{}}"#,
                "host",
            ),
        ] {
            let err = TopicInstallBundle::from_json(body).expect_err(missing);
            assert!(
                matches!(err, BundleError::Parse(ref m) if m.contains(missing)),
                "{missing}: {err}"
            );
        }
    }

    #[test]
    fn digest_expectations_are_exactly_lowercase_and_unpadded() {
        let upper = HEX.to_ascii_uppercase();
        for bad in [
            format!("sha256:{upper}"),
            format!("sha256: {HEX}"),
            format!("sha256:{HEX} "),
            format!(" sha256:{HEX}"),
            format!("sha256:{}", &HEX[..63]),
            format!("sha256:{HEX}0"),
            format!("SHA256:{HEX}"),
            HEX.to_owned(),
        ] {
            for field in ["rlm_image_digest", "experiment_image_digest", "pack_digest"] {
                let mut bundle = tb4();
                match field {
                    "rlm_image_digest" => bundle.host.rlm_image_digest = Some(bad.clone()),
                    "experiment_image_digest" => {
                        bundle.host.experiment_image_digest = Some(bad.clone());
                    }
                    _ => bundle.host.pack_digest = Some(bad.clone()),
                }
                assert!(
                    matches!(
                        bundle.validate_shape(),
                        Err(BundleError::BadDigest { field: f, .. }) if f == field
                    ),
                    "{field}={bad:?} must be refused, not silently accepted"
                );
            }
        }
        tb4().validate_shape().expect("lowercase hex validates");
        assert!(is_lower_hex64(HEX));
        assert!(!is_lower_hex64(&upper));
        assert!(!is_lower_hex64(&format!(" {HEX}")));
    }

    /// The host block may only agree with the document.
    #[test]
    fn a_host_expectation_that_contradicts_the_document_is_refused() {
        let other = format!("sha256:{}", "cd".repeat(32));
        let mut bundle = tb4();
        bundle.host.pack_digest = Some(other.clone());
        let err = bundle.validate_shape().expect_err("contradicting pack");
        assert!(
            matches!(
                err,
                BundleError::HostContradictsDocument {
                    field: "pack_digest",
                    ref got,
                    ..
                } if *got == other
            ),
            "{err:?}"
        );
        assert!(err.to_string().contains("contradicts"), "{err}");
    }

    #[test]
    fn a_runner_and_its_pack_travel_together() {
        let mut no_pack = tb4();
        no_pack.host.pack_digest = None;
        let err = no_pack.validate_shape().expect_err("runner without pack");
        assert!(
            matches!(err, BundleError::RunnerWithoutPack { ref runner_id }
                if runner_id == "rlm_fc_in_guest_harbor"),
            "{err:?}"
        );
        assert!(err.to_string().contains("never invented"), "{err}");

        // A pack pinned for a topic that selects no runner is refused: the
        // bundle would stage something nothing reads.
        let mut orphan = tb4();
        orphan
            .topic
            .constraints
            .params
            .remove(proof_experiment::PARAM_RUNNER);
        orphan
            .topic
            .constraints
            .params
            .remove(proof_experiment::PARAM_PACK_DIGEST);
        assert!(matches!(
            orphan.validate_shape(),
            Err(BundleError::PackWithoutRunner)
        ));

        // No runner, no pack: the harvest-family shape is legal.
        let mut harvest = tb4();
        harvest
            .topic
            .constraints
            .params
            .remove(proof_experiment::PARAM_RUNNER);
        harvest
            .topic
            .constraints
            .params
            .remove(proof_experiment::PARAM_PACK_DIGEST);
        harvest.host.pack_digest = None;
        harvest.topic.metric.family = MetricFamily::Nll;
        harvest.topic.metric.custom_id = String::new();
        harvest.topic.metric.primary = proof_task::PRIMARY_HOLDOUT_NLL.into();
        harvest.host.custom_ids_entry = None;
        harvest
            .validate_shape()
            .expect("a topic may select no runner");
        let plan = harvest.plan(InstallEnvironment::Metal).expect("plan");
        assert!(plan.runner_id.is_none());
        assert!(plan.pack_dir_env.is_none());
    }

    /// A malformed binding in the document is refused, never ignored: a topic
    /// that half-selects a backend must not install on another path.
    #[test]
    fn a_malformed_document_binding_is_refused() {
        let mut bundle = tb4();
        bundle.topic.constraints.params.insert(
            proof_experiment::PARAM_PACK_DIGEST.into(),
            "not-a-digest".into(),
        );
        let err = bundle.validate_shape().expect_err("bad pack digest");
        assert!(matches!(err, BundleError::Binding(_)), "{err:?}");

        // A runner id that is not a custom-id shape is refused by the shared
        // binding reader rather than silently treated as "no runner".
        let mut bad_runner = tb4();
        bad_runner
            .topic
            .constraints
            .params
            .insert(proof_experiment::PARAM_RUNNER.into(), "Not A Runner".into());
        assert!(matches!(
            bad_runner.validate_shape(),
            Err(BundleError::Binding(_))
        ));

        // Dropping the runner but leaving its pack behind is the half-selected
        // shape: nothing runs the pack, so the bundle refuses rather than
        // staging it.
        let mut no_runner = tb4();
        no_runner
            .topic
            .constraints
            .params
            .remove(proof_experiment::PARAM_RUNNER);
        assert!(matches!(
            no_runner.validate_shape(),
            Err(BundleError::PackWithoutRunner)
        ));
    }

    #[test]
    fn an_open_custom_topic_needs_its_id_registered_in_the_host_block() {
        let mut bundle = tb4();
        bundle.host.custom_ids_entry = Some("some_other_metric".into());
        let err = bundle.validate_shape().expect_err("id not registered");
        assert!(
            matches!(err, BundleError::CustomIdNotRegistered { ref custom_id }
                if custom_id == "tbench"),
            "{err:?}"
        );
        assert!(err.to_string().contains("503"), "{err}");

        // The id list is parsed the same way `proof-challenge` parses it.
        bundle.host.custom_ids_entry = Some("other_metric, tbench ,third".into());
        bundle.validate_shape().expect("one entry is enough");
        assert_eq!(
            bundle.registered_custom(),
            ["other_metric", "tbench", "third"]
        );
        assert!(parse_custom_ids("  ").is_empty());
    }

    #[test]
    fn environments_are_exactly_the_two_install_targets() {
        assert_eq!(INSTALL_ENVIRONMENTS, ["staging", "metal"]);
        for (word, want) in [
            ("staging", InstallEnvironment::Staging),
            ("METAL", InstallEnvironment::Metal),
            (" metal ", InstallEnvironment::Metal),
        ] {
            assert_eq!(word.parse::<InstallEnvironment>().expect(word), want);
        }
        assert!("prod".parse::<InstallEnvironment>().is_err());
        assert_eq!(InstallEnvironment::Staging.as_str(), "staging");
    }

    #[test]
    fn a_bundle_for_another_target_is_refused_not_coerced() {
        let mut bundle = tb4();
        bundle.environment = InstallEnvironment::Staging;
        let err = bundle
            .plan(InstallEnvironment::Metal)
            .expect_err("staging bundle on metal");
        assert!(
            matches!(
                err,
                BundleError::EnvironmentMismatch {
                    bundle: InstallEnvironment::Staging,
                    requested: InstallEnvironment::Metal,
                }
            ),
            "{err:?}"
        );
        assert!(err.to_string().contains("staging"), "{err}");
    }

    #[test]
    fn the_digest_ignores_formatting_and_key_order() {
        let bundle = tb4();
        let body = serde_json::to_string(&bundle).expect("json");
        let reparsed = TopicInstallBundle::from_json(&body).expect("parse");
        assert_eq!(
            bundle.digest().expect("digest"),
            reparsed.digest().expect("digest"),
            "a round trip is the same bundle"
        );

        // Any real change is a different install, so a different digest.
        let mut changed = bundle.clone();
        changed.host.custom_ids_entry = Some("tbench,extra".into());
        assert_ne!(
            bundle.digest().expect("a"),
            changed.digest().expect("changed"),
            "a changed bundle must not hash the same"
        );
        let mut renamed = bundle;
        renamed.display_name = "Other".into();
        assert_ne!(
            reparsed.digest().expect("b"),
            renamed.digest().expect("renamed"),
            "the label is part of the identity"
        );
    }

    /// `pack_dir` names a directory, so it is checked as a path — and it never
    /// carries the digest, which travels in the document.
    #[test]
    fn pack_dir_is_a_path_not_a_digest() {
        for bad in ["", "relative/packs", "/var/../etc", "sha256:abc"] {
            let mut bundle = tb4();
            bundle.host.pack_dir = Some(bad.into());
            assert!(
                matches!(bundle.validate_shape(), Err(BundleError::BadPackDir(_))),
                "{bad:?} must be refused as a pack dir"
            );
        }
        let mut no_dir = tb4();
        no_dir.host.pack_dir = None;
        no_dir.validate_shape().expect("a pack dir is optional");
        let plan = no_dir.plan(InstallEnvironment::Metal).expect("plan");
        assert!(
            !plan.host_env.iter().any(|e| e.name == ENV_PACK_DIR),
            "no directory declared means no pack-dir env line: {:?}",
            plan.host_env
        );

        // With a directory, the value is the path and the digest is only in
        // the explanation.
        let plan = tb4().plan(InstallEnvironment::Metal).expect("plan");
        let pack = plan
            .host_env
            .iter()
            .find(|e| e.name == ENV_PACK_DIR)
            .expect("pack dir line");
        assert_eq!(pack.value, "/var/lib/proof/packs");
        assert!(!pack.value.starts_with("sha256:"), "{pack:?}");
        assert!(pack.why.contains(&digest()), "{pack:?}");
    }

    #[test]
    fn a_plan_is_serialisable_for_the_dry_run_json_output() {
        let plan = tb4().plan(InstallEnvironment::Metal).expect("plan");
        let body = serde_json::to_string(&plan).expect("json");
        assert!(body.contains(r#""topic_id":"tb4""#), "{body}");
        assert!(
            body.contains(r#""publish_route":"POST /v1/admin/proof/topics""#),
            "{body}"
        );
        assert!(body.contains(r#""environment":"metal""#), "{body}");
        let round: TopicInstallPlan = serde_json::from_str(&body).expect("round trip");
        assert_eq!(round, plan);
    }
}
