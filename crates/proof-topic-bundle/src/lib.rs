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
//! target, which host env must agree with it, and what the topic's RLM is
//! asked to install. That is this bundle. It **references** the document,
//! **cross-checks** the host expectations against it, and **carries** the
//! RLM-owned section verbatim; it never restates a binding in a second place
//! that could drift.
//!
//! # The RLM owns topic behavior; this crate does not
//!
//! Topics are **RLM-based and autonomous**. The admin CLI's job is to hand
//! control to the topic's RLM — it asks the RLM to install and set itself up.
//! Everything that makes a topic *that* topic belongs to the bundle's
//! [`RlmSection`]: its anti-cheat **rules**, the **SQL migrations** it needs,
//! the **APIs** it exposes, its **submission format**, and its **scoring**.
//!
//! Rust never interprets any of it. This crate checks the section's *shape*
//! (an object, bounded) and carries it byte-for-byte; it does not know what a
//! rule, a migration, an API, or a scoring function *means*. That is the
//! whole point: no `if topic == …` branch, no compiled-in rule list, no
//! metric or submit format baked into challenge, gateway, or orchestrator
//! code. A topic's behavior travels in its signed document and its RLM
//! section, never in this binary.
//!
//! Consequence for tests and fixtures: the seed slug `tb4` and its temporary
//! alias `tbench` are **strings** that appear in test fixtures and operator
//! examples. They are never a condition in logic.
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
use serde_json::value::RawValue;

/// Only accepted `schema_version`.
pub const BUNDLE_SCHEMA_VERSION: u32 = 1;

/// Longest legal `display_name`.
pub const MAX_DISPLAY_NAME_LEN: usize = 128;

/// Every key the bundle schema accepts, sorted. The schema is this Rust type
/// (`deny_unknown_fields`), not a second document that could drift from it:
/// this list is what a test pins, so adding or removing a key is a deliberate
/// edit here rather than a silent widening of what an operator may write.
pub const BUNDLE_KEYS: [&str; 7] = [
    "aliases",
    "display_name",
    "environment",
    "host",
    "rlm",
    "schema_version",
    "topic",
];

/// Keys with no `serde` default: a bundle that omits one is a parse error
/// naming the field, never an empty value that fails later.
///
/// `aliases` and `rlm` are absent deliberately: a bundle with neither is the
/// common shape, and both default to "nothing extra".
pub const REQUIRED_BUNDLE_KEYS: [&str; 5] = [
    "display_name",
    "environment",
    "host",
    "schema_version",
    "topic",
];

/// Most aliases one bundle may declare.
pub const MAX_ALIASES: usize = 8;

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

/// The RLM jobs an install drives, in order, for operator output.
///
/// These are the **existing** RLM lifecycle steps (`proof-rlm-scorer`
/// `TopicSetup`): the RLM is asked to provision, write its rules, and seal a
/// baseline. Naming them here is documentation for the operator; the CLI does
/// not run them, and none of them is topic-specific.
pub const RLM_INSTALL_JOBS: [&str; 3] = ["provision", "propose_rules", "baseline"];

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
    /// An alias is not a usable slug, or is the topic's own id.
    #[error(
        "alias {alias:?} is not usable: {why} (an alias is a topic slug, \
         `[a-z0-9][a-z0-9-]{{1,62}}`, and may never be the topic's own id — that is a second \
         spelling of the same key in one lookup)"
    )]
    BadAlias {
        /// The alias the bundle declared.
        alias: String,
        /// Why it is not usable.
        why: &'static str,
    },
    /// The same alias is declared twice.
    #[error("alias {0:?} is declared twice")]
    DuplicateAlias(String),
    /// More aliases than a bundle may declare.
    #[error("bundle declares {got} aliases, at most {MAX_ALIASES} are installed")]
    TooManyAliases {
        /// How many it declared.
        got: usize,
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
    /// An RLM section field is not an object or array.
    #[error("rlm.{field} must be a JSON object or array, got {got}")]
    RlmNotObject {
        /// Which field.
        field: String,
        /// What it was.
        got: &'static str,
    },
    /// An RLM part was written as an explicit `null`.
    #[error(
        "rlm.{field} is an explicit null; a part the operator wrote is never silently dropped \
         — remove the key instead"
    )]
    RlmExplicitNull {
        /// Which part.
        field: String,
    },
    /// The RLM section is larger than the bound.
    #[error("rlm section is {0} bytes of canonical JSON, at most {MAX_RLM_BYTES} are allowed")]
    RlmTooLarge(usize),
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

/// What the topic's **RLM** is asked to install — opaque to Rust.
///
/// Topics are RLM-based and autonomous. Everything topic-specific lives here,
/// owned by the bundle and consumed by the RLM inside its VM: the anti-cheat
/// **rules**, the **SQL migrations** the topic needs, the **APIs** it exposes,
/// its **submission format**, and its **scoring**.
///
/// The whole object is held as **raw JSON text** ([`RawValue`]), not a parsed
/// value. That is deliberate and load-bearing: parsing and re-serializing
/// reorders keys, collapses duplicate keys, and normalises whitespace, so the
/// bytes handed to the RLM would not be the bytes the operator wrote. This
/// crate carries the text it was given — including the enclosing object's own
/// key order and any duplicate keys inside it.
///
/// [`Self::validate_shape`] *parses a copy* to bound and shape-check the
/// section (an object; each known part an object or array; not an explicit
/// `null`). Checking is not transforming: the hand-off is always
/// [`Self::raw`], the original bytes. Nothing here is validated semantically,
/// and nothing here may become a branch in challenge, gateway, or
/// orchestrator code — a part Rust has never heard of goes in the section
/// rather than requiring a code change.
#[derive(Debug, Clone)]
pub struct RlmSection {
    /// The section verbatim: the authoritative hand-off bytes.
    raw: Box<RawValue>,
}

/// The parts a section may name, for the shape check and for error naming.
///
/// These are the parts the bundle owns per the architecture. Naming them here
/// makes the shape reviewable; it does **not** make this crate understand
/// them, and an unrecognised key is not this crate's business to reject on
/// semantic grounds — see [`RlmSection::validate_shape`].
pub const RLM_KEYS: [&str; 5] = [
    "apis",
    "migrations",
    "rules",
    "scoring",
    "submission_format",
];

/// Largest RLM section, in bytes.
///
/// A bound, not a schema: it stops a bundle from smuggling an unbounded blob
/// through the install path, and it says nothing about what the content is.
pub const MAX_RLM_BYTES: usize = 256 * 1024;

/// The empty section, which is what a bundle with no `rlm` key carries.
const EMPTY_RLM: &str = "{}";

impl Default for RlmSection {
    /// The empty section (`{}`).
    ///
    /// `EMPTY_RLM` is a valid JSON object by construction, so the parse cannot
    /// fail. The workspace denies `expect` outside tests, and the alternative
    /// (a fallback that also parses) would be noise around an unreachable arm
    /// — so this documents the invariant instead of hiding it.
    #[allow(clippy::expect_used)]
    fn default() -> Self {
        Self {
            raw: RawValue::from_string(EMPTY_RLM.to_owned()).expect("`{}` is valid JSON"),
        }
    }
}

impl PartialEq for RlmSection {
    fn eq(&self, other: &Self) -> bool {
        self.raw() == other.raw()
    }
}

impl Eq for RlmSection {}

impl Serialize for RlmSection {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // `RawValue` writes its bytes through untouched.
        self.raw.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RlmSection {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Capture the whole value as text; `validate_shape` checks it later,
        // so a parse error never has to be reconstructed from a typed value.
        Ok(Self {
            raw: Box::<RawValue>::deserialize(deserializer)?,
        })
    }
}

impl RlmSection {
    /// The section verbatim — the bytes handed to the RLM.
    #[must_use]
    pub fn raw(&self) -> &str {
        self.raw.get()
    }

    /// Build from raw text, the way a bundle file supplies it.
    ///
    /// # Errors
    ///
    /// [`BundleError::Canonicalize`] when the text is not valid JSON.
    pub fn from_raw(text: &str) -> Result<Self, BundleError> {
        Ok(Self {
            raw: RawValue::from_string(text.to_owned())
                .map_err(|e| BundleError::Canonicalize(e.to_string()))?,
        })
    }

    /// Whether the section names nothing at all (`{}`).
    ///
    /// A malformed section reads as empty here; [`Self::validate_shape`] is
    /// what refuses it, so this can only ever *withhold* a hand-off.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(self.raw())
            .is_ok_and(|m| m.is_empty())
    }

    /// Shape only: a bounded JSON object whose named parts are objects or
    /// arrays, and no part is an explicit `null`.
    ///
    /// Deliberately says nothing about the *content* — a rule list this crate
    /// does not recognise is not an error, because recognising it would mean
    /// this crate knows the topic. The parse here is a **check**; the hand-off
    /// stays [`Self::raw`].
    fn validate_shape(&self) -> Result<(), BundleError> {
        let text = self.raw();
        if text.len() > MAX_RLM_BYTES {
            return Err(BundleError::RlmTooLarge(text.len()));
        }
        let trimmed = text.trim();
        if !trimmed.starts_with('{') {
            return Err(BundleError::RlmNotObject {
                field: "rlm".to_owned(),
                got: raw_kind(trimmed),
            });
        }
        let parsed: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(text).map_err(|e| BundleError::Parse(e.to_string()))?;
        for (key, value) in &parsed {
            // An explicit null is refused rather than dropped: the operator
            // wrote it, so silently discarding it would change the install.
            if value.is_null() {
                return Err(BundleError::RlmExplicitNull { field: key.clone() });
            }
            // Only the parts this crate names are shape-checked; a part it has
            // never heard of is the RLM's business, not an error.
            if RLM_KEYS.contains(&key.as_str()) && !value.is_object() && !value.is_array() {
                return Err(BundleError::RlmNotObject {
                    field: key.clone(),
                    got: raw_kind(&value.to_string()),
                });
            }
        }
        Ok(())
    }
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
    /// Temporary compatibility slugs this topic answers to, if the bundle
    /// declares any.
    ///
    /// Owner default: the first topic's slug is `tb4` with `tbench` as a
    /// **temporary** alias so existing miner links keep resolving. An alias
    /// is not topic data — the topic's identity is its signed document's
    /// `id` — so this is a bundle field that becomes a `proof_topic_alias`
    /// row, and retiring it is deleting the row.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// What the topic's RLM installs. Opaque to Rust: see [`RlmSection`].
    #[serde(default)]
    pub rlm: RlmSection,
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
    /// Temporary compatibility aliases this install will point at the topic,
    /// in declaration order. Empty when the bundle declares none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// In-guest runner the document selects, when it selects one.
    pub runner_id: Option<String>,
    /// Experiment pack digest the document pins, when it pins one.
    pub pack_digest: Option<String>,
    /// The existing admin route that publishes this document.
    pub publish_route: String,
    /// What the CLI hands the RLM: the install section, verbatim.
    ///
    /// Present only when the bundle carries one. This is the hand-off — the
    /// admin CLI asks the RLM to install and set the topic up; it does not
    /// interpret, rewrite, or partially apply any of it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rlm_install: Option<RlmSection>,
    /// The RLM job kinds this install is expected to drive, for operator
    /// output only. Derived from the existing RLM lifecycle, not from the
    /// bundle's contents: this crate still reads none of it.
    pub rlm_jobs: Vec<String>,
    /// Operator env lines this install needs, in the order to set them.
    pub host_env: Vec<HostEnvVar>,
    /// Where the pack is staged on the KVM host, when a pack is pinned.
    pub pack_dir_env: Option<String>,
    /// `sha256:<hex>` over the canonical bundle.
    pub bundle_digest: String,
}

/// Name a raw JSON part's kind, for an error that says what arrived.
fn raw_kind(text: &str) -> &'static str {
    let t = text.trim();
    match t.chars().next() {
        Some('"') => "a string",
        Some('{') => "an object",
        Some('[') => "an array",
        Some('t' | 'f') => "a boolean",
        Some('n') => "null",
        _ => "a number",
    }
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
        self.check_aliases()?;
        // Shape only. The RLM section's *content* is the topic's business:
        // this crate carries it, never interprets it.
        self.rlm.validate_shape()?;
        self.cross_check_host()
    }

    /// Whether every declared alias is a usable, distinct, non-self slug.
    ///
    /// An alias is a **lookup key**: it must be a topic slug shape, it must
    /// not be the topic's own id (that is a second spelling of one key), and
    /// it must not repeat within the bundle. Whether it collides with another
    /// *published* topic is not knowable here — the store and the
    /// `0024` trigger decide that at write time, fail-closed.
    fn check_aliases(&self) -> Result<(), BundleError> {
        if self.aliases.len() > MAX_ALIASES {
            return Err(BundleError::TooManyAliases {
                got: self.aliases.len(),
            });
        }
        let mut seen: Vec<&str> = Vec::with_capacity(self.aliases.len());
        for alias in &self.aliases {
            let a = alias.trim();
            if a.is_empty() {
                return Err(BundleError::BadAlias {
                    alias: alias.clone(),
                    why: "it is empty",
                });
            }
            if a.len() > 63 {
                return Err(BundleError::BadAlias {
                    alias: alias.clone(),
                    why: "it is longer than 63 characters",
                });
            }
            let mut chars = a.chars();
            let head_ok = chars
                .next()
                .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
            let rest_ok = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
            if !head_ok || !rest_ok {
                return Err(BundleError::BadAlias {
                    alias: alias.clone(),
                    why: "it must be `[a-z0-9][a-z0-9-]{1,62}`",
                });
            }
            if a == self.topic.id {
                return Err(BundleError::BadAlias {
                    alias: alias.clone(),
                    why: "it is the topic's own id",
                });
            }
            if seen.contains(&a) {
                return Err(BundleError::DuplicateAlias(a.to_owned()));
            }
            seen.push(a);
        }
        Ok(())
    }

    /// The aliases this bundle installs, trimmed, in declaration order.
    #[must_use]
    pub fn aliases(&self) -> Vec<String> {
        self.aliases
            .iter()
            .map(|a| a.trim().to_owned())
            .filter(|a| !a.is_empty())
            .collect()
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
            aliases: self.aliases(),
            runner_id: binding.as_ref().map(|b| b.runner.clone()),
            pack_digest: binding.as_ref().map(|b| b.pack.digest.clone()),
            publish_route: PUBLISH_ROUTE.to_owned(),
            rlm_install: (!self.rlm.is_empty()).then(|| self.rlm.clone()),
            rlm_jobs: RLM_INSTALL_JOBS.iter().map(|s| (*s).to_owned()).collect(),
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
            aliases: vec!["tbench".into()],
            rlm: RlmSection::default(),
        }
    }

    /// A raw section from text, the way a bundle file supplies it.
    fn rlm(text: &str) -> RlmSection {
        RlmSection::from_raw(text).expect("raw json")
    }

    /// A section carrying all five RLM-owned parts.
    fn rlm_section() -> RlmSection {
        rlm(
            r#"{"rules": [{"id": "no_short_circuit", "text": "run the task"}],
                "migrations": [{"name": "0001_scratch", "sql": "CREATE TABLE s (id TEXT)"}],
                "apis": [{"path": "/v1/topic/status", "method": "GET"}],
                "submission_format": {"kind": "tar", "max_bytes": 5242880},
                "scoring": {"primary": "success_rate", "epsilon_rel": 0.05}}"#,
        )
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

    /// The RLM section is **opaque**: this crate carries it and never interprets
    /// it. The test proves the carry is byte-exact and that content this crate has
    /// never heard of is not an error — recognising it would mean this crate
    /// knows the topic, which is exactly the hardcoding the boundary prevents.
    #[test]
    fn the_rlm_section_is_carried_verbatim_and_never_interpreted() {
        let mut bundle = tb4();
        bundle.rlm = rlm_section();
        bundle
            .validate_shape()
            .expect("an opaque section validates");
        let plan = bundle.plan(InstallEnvironment::Metal).expect("plan");

        let carried = plan
            .rlm_install
            .as_ref()
            .expect("the section is handed over");
        assert_eq!(carried, &bundle.rlm, "handed over unchanged");
        assert_eq!(
            carried.raw(),
            bundle.rlm.raw(),
            "the hand-off is the original text"
        );

        // Content Rust has never seen is still not an error.
        let mut exotic = tb4();
        exotic.rlm =
            rlm(r#"{"some_future_metric_this_build_has_never_heard_of": {"weight": 0.7}}"#);
        exotic
            .validate_shape()
            .expect("unknown content is not a validation error");

        assert_eq!(plan.rlm_jobs, ["provision", "propose_rules", "baseline"]);

        let empty = tb4();
        empty.validate_shape().expect("an absent section is fine");
        assert!(
            empty
                .plan(InstallEnvironment::Metal)
                .expect("plan")
                .rlm_install
                .is_none(),
            "a bundle with no RLM section hands over nothing"
        );
    }

    /// The hand-off must preserve the **bytes** the operator wrote — including the
    /// enclosing object's own key order and duplicate keys inside it.
    ///
    /// Parsing the section into a `Value` and re-serializing would reorder its
    /// keys, collapse duplicates, and normalise whitespace, so the RLM would
    /// receive something other than what was signed off. This is checked at the
    /// object level, not only inside a named part: an earlier revision preserved
    /// the parts but rebuilt the object around them.
    #[test]
    fn the_hand_off_preserves_key_order_duplicates_and_whitespace() {
        // Object keys deliberately NOT in `RLM_KEYS` order, plus a duplicate key
        // and significant inner whitespace.
        let awkward = concat!(
            r#"{"scoring": {"b": 1, "a": 2, "a": 3, "sp": "x   y"}, "#,
            r#""rules": [{"id": "r", "text": "t"}], "apis": []}"#
        );
        let fixture = serde_json::to_string(&tb4()).expect("fixture json");
        // Splice the section in as **text**, so the test does not itself round-trip
        // it through a `Value` (which is the lossy path under test).
        let body = fixture.replacen("\"rlm\":{}", &format!("\"rlm\":{awkward}"), 1);
        assert!(body.contains(awkward), "the splice must have landed");
        let bundle = TopicInstallBundle::from_json(&body).expect("parse");
        bundle.validate_shape().expect("validates");
        assert_eq!(
            bundle.rlm.raw(),
            awkward,
            "the exact bytes must survive parsing, object order included"
        );

        // And through a serialize/parse round trip, as the plan's JSON output does.
        let plan = bundle.plan(InstallEnvironment::Metal).expect("plan");
        let plan_json = serde_json::to_string(&plan).expect("plan json");
        let reparsed: TopicInstallPlan = serde_json::from_str(&plan_json).expect("reparse plan");
        assert_eq!(
            reparsed.rlm_install.as_ref().expect("carried").raw(),
            awkward,
            "the exact bytes must survive the plan's own JSON"
        );
    }

    /// An explicit `null` is refused, not silently dropped.
    ///
    /// Folding `"rules": null` into "absent" would mean the operator signed off one
    /// bundle and the RLM received another — or, worse, that the whole hand-off
    /// vanished. The section keeps the text, and the shape check refuses it.
    #[test]
    fn an_explicit_null_rlm_part_is_refused_not_dropped() {
        let fixture = serde_json::to_string(&tb4()).expect("fixture json");
        let body = fixture.replacen("\"rlm\":{}", "\"rlm\":{\"rules\":null}", 1);
        let bundle = TopicInstallBundle::from_json(&body).expect("parse");
        assert_eq!(bundle.rlm.raw(), r#"{"rules":null}"#, "kept, not folded");
        let err = bundle
            .validate_shape()
            .expect_err("an explicit null is refused");
        assert!(
            matches!(err, BundleError::RlmExplicitNull { ref field } if field == "rules"),
            "{err:?}"
        );
        assert!(err.to_string().contains("never silently dropped"), "{err}");
    }

    /// Only the *shape* of the section is checked, and only to keep it bounded.
    #[test]
    fn the_rlm_section_is_shape_checked_but_not_semantically_validated() {
        // A named part must be an object or array.
        for key in RLM_KEYS {
            let mut bundle = tb4();
            bundle.rlm = rlm(&format!(r#"{{"{key}": "a bare string"}}"#));
            let err = bundle
                .validate_shape()
                .expect_err(&format!("{key} must be an object or array"));
            assert!(
                matches!(err, BundleError::RlmNotObject { ref field, .. } if field == key),
                "{key}: {err:?}"
            );
        }

        // The section itself must be an object.
        let mut scalar = tb4();
        scalar.rlm = rlm("[1, 2, 3]");
        assert!(matches!(
            scalar.validate_shape(),
            Err(BundleError::RlmNotObject { .. })
        ));

        // An unrecognised key is NOT an error: it is the RLM's business.
        let mut unknown = tb4();
        unknown.rlm = rlm(r#"{"a_part_this_build_has_never_heard_of": "opaque"}"#);
        unknown
            .validate_shape()
            .expect("an unknown part is data, not an error");

        // The bound is on the section's own size.
        let mut huge = tb4();
        huge.rlm = rlm(&format!(
            r#"{{"scoring": {{"pad": "{}"}}}}"#,
            "x".repeat(MAX_RLM_BYTES)
        ));
        let err = huge.validate_shape().expect_err("oversized section");
        let BundleError::RlmTooLarge(reported) = err else {
            panic!("expected RlmTooLarge, got {err:?}");
        };
        assert!(reported > MAX_RLM_BYTES, "{reported}");
    }

    /// The topic slug and its alias are **strings**, never conditions.
    ///
    /// This is the guard against the hardcoding the architecture forbids: the
    /// seed ids may appear in fixtures and examples, but no logic may branch on
    /// them, and this crate must not know any topic by name. The check is on the
    /// crate's own non-test source, so a future edit that adds `if topic == "tb4"`
    /// fails here.
    #[test]
    fn no_topic_literal_appears_in_this_crates_logic() {
        const SOURCE: &str = include_str!("lib.rs");
        // The test module is where fixtures legitimately name the seed ids, and
        // a doc comment may *explain* the rule — so the check runs on the
        // non-test source with comment lines stripped. That is precisely "no
        // topic literal in logic": a string in a `let`, `match`, or `if` is
        // caught; prose about the boundary is not.
        let strip = |s: &str| -> String {
            s.lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let logic = strip(
            SOURCE
                .split("#[cfg(test)]")
                .next()
                .expect("non-test source"),
        );
        assert!(
            !logic.contains("tb4") && !logic.contains("tbench"),
            "topic ids belong in fixtures and signed documents, never in logic"
        );
        // The same guard for the parts a topic would otherwise be tempted to bake
        // in: this crate must not name a metric, a task, or a benchmark.
        for forbidden in ["terminal-bench", "harbor", "success_rate"] {
            assert!(
                !logic.to_lowercase().contains(forbidden),
                "{forbidden} must not be compiled into this crate"
            );
        }
        // Guard the guard: a literal in real code must still be caught even
        // though a comment beside it is filtered out.
        assert!(
            strip("let topic = \"tb4\"; // fixture").contains("tb4"),
            "the comment filter must not hide a literal in code"
        );
        // The RLM-owned key *names* are the one thing it may know, because they
        // are the section's shape.
        for key in RLM_KEYS {
            assert!(logic.contains(key), "the section shape must name {key}");
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

    /// Aliases are lookup keys, so they are shape-checked here and their
    /// collisions with *published* topics are left to the store's fail-closed
    /// guard — the bundle cannot know what is published.
    #[test]
    fn aliases_are_slugs_that_are_neither_the_topic_nor_repeated() {
        // The Owner default shape: slug `tb4`, temporary alias `tbench`.
        let bundle = tb4();
        assert_eq!(bundle.aliases(), ["tbench"]);
        bundle
            .validate_shape()
            .expect("the default alias validates");
        let plan = bundle.plan(InstallEnvironment::Metal).expect("plan");
        assert_eq!(plan.aliases, ["tbench"]);

        // A bundle with none is the common shape, and the plan carries none.
        let mut bare = tb4();
        bare.aliases = Vec::new();
        bare.validate_shape().expect("no aliases is fine");
        assert!(bare.aliases().is_empty());
        assert!(bare
            .plan(InstallEnvironment::Metal)
            .expect("plan")
            .aliases
            .is_empty());

        for bad in [
            "Tbench",   // upper case is not a slug
            "t bench",  // no spaces
            "-tbench",  // must start alphanumeric
            "tbench-",  // trailing hyphen is a slug, so this one is fine…
            "tbench/x", // no path separators
            "tb4",      // the topic's own id
            "",
        ] {
            let mut b = tb4();
            b.aliases = vec![bad.into()];
            // A trailing hyphen is a legal slug (`[a-z0-9][a-z0-9-]{1,62}`),
            // so it must *not* be refused.
            if bad == "tbench-" {
                b.validate_shape()
                    .unwrap_or_else(|e| panic!("{bad:?} is legal: {e}"));
                continue;
            }
            assert!(
                matches!(b.validate_shape(), Err(BundleError::BadAlias { .. })),
                "{bad:?} must be refused as an alias"
            );
        }

        let mut dup = tb4();
        dup.aliases = vec!["tbench".into(), "tbench".into()];
        assert!(matches!(
            dup.validate_shape(),
            Err(BundleError::DuplicateAlias(ref a)) if a == "tbench"
        ));

        let mut many = tb4();
        many.aliases = (0..=MAX_ALIASES).map(|i| format!("alias-{i}")).collect();
        assert!(matches!(
            many.validate_shape(),
            Err(BundleError::TooManyAliases { got }) if got == MAX_ALIASES + 1
        ));
        let mut just_enough = tb4();
        just_enough.aliases = (0..MAX_ALIASES).map(|i| format!("alias-{i}")).collect();
        just_enough
            .validate_shape()
            .expect("exactly the maximum is allowed");

        // An alias is a lookup key, never topic data: the bundle's own key
        // list still forbids a second place to restate a binding.
        let value = serde_json::to_value(tb4()).expect("json");
        assert_eq!(value["aliases"], serde_json::json!(["tbench"]));
        assert_eq!(value["topic"]["id"], "tb4");
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
