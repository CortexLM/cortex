//! Proof **topic install bundle**: the JSON document an operator installs a
//! topic from.
//!
//! A topic's *scoring contract* is its signed topic document (see
//! `proof-task`). The *install* is a separate operator record: which runner
//! the topic names, which RLM and experiment images and which experiment pack
//! it is pinned to, how much concurrency it may use, and whether it is live.
//! This crate is the shape of that record, the checks it has to pass before
//! anything is written, and the canonical digest a later slice can pin.
//!
//! P0 scope (dynamic-topics skeleton): parse, validate, digest, and describe.
//! This crate never touches the database, the network, or the filesystem, and
//! it never enables anything. The CLI that drives it (`bins/proof-admin`)
//! writes a row with `enabled = false` and nothing in this repository reads
//! that row on a scoring path yet — the routes (P1), the allocator (P2), the
//! full install (P3), and the removal of the compiled-in `tbench` bindings
//! (P4) are later slices.
//!
//! Three rules carry the fail-closed posture:
//!
//! - **Unknown keys are refused.** A binding this build does not understand
//!   is a binding nothing enforces, so `deny_unknown_fields` rejects it at
//!   parse rather than installing a topic that half-works.
//! - **A digest is never invented.** Every pin is `sha256:<64 hex>` or it is
//!   absent; absent means "not pinned", which every later slice must read as
//!   fail-closed (an unpinned topic never boots), never as a default.
//! - **A runner without a pack is refused.** An in-guest runner with nothing
//!   to run is a job that cannot score, so the pair travels together or not
//!   at all — the same rule the signed topic's `constraints.params` carries.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::doc_markdown
)]

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Only accepted `schema_version`.
pub const BUNDLE_SCHEMA_VERSION: u32 = 1;

/// Longest legal `display_name`.
pub const MAX_DISPLAY_NAME_LEN: usize = 128;

/// Most aliases one topic may carry.
pub const MAX_ALIASES: usize = 8;

/// Largest canonical `config` object, in bytes.
pub const MAX_CONFIG_BYTES: usize = 16 * 1024;

/// Largest `version` / `n_concurrent` a bundle may carry.
///
/// The row's columns are `INTEGER`, so a value above this would have to be
/// clamped on write — and a clamped row would disagree with the validated,
/// digest-covered bundle an operator reviewed. Out of range is a reject, never
/// a silent rewrite.
pub const MAX_INT_COLUMN: u32 = i32::MAX as u32;

/// Install targets, in the order the CLI offers them.
pub const INSTALL_ENVIRONMENTS: [&str; 2] = ["staging", "metal"];

/// Every key the bundle schema accepts, sorted. The schema is this Rust type
/// (`deny_unknown_fields`), not a second document that could drift from it:
/// this list is what a test pins, so adding or removing a key is a deliberate
/// edit here rather than a silent widening of what an operator may write.
pub const BUNDLE_KEYS: [&str; 13] = [
    "aliases",
    "config",
    "display_name",
    "environment",
    "n_concurrent",
    "pack_digest",
    "pin_experiment",
    "pin_rlm",
    "runner_id",
    "schema_version",
    "sealed_custom_value",
    "topic_id",
    "version",
];

/// Keys with no `serde` default: a bundle that omits one is a parse error
/// naming the field, never an empty string that fails later.
pub const REQUIRED_BUNDLE_KEYS: [&str; 5] = [
    "display_name",
    "environment",
    "schema_version",
    "topic_id",
    "version",
];

/// Prefix of every pin digest.
pub const DIGEST_PREFIX: &str = "sha256:";

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
    /// Wire word (`staging` / `metal`), which is also the DB value.
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
    /// `topic_id` is not `[a-z0-9][a-z0-9-]{1,62}`.
    #[error("topic_id {0:?} must match [a-z0-9][a-z0-9-]{{1,62}} (a hyphen slug)")]
    BadTopicId(String),
    /// `display_name` is empty or oversized.
    #[error("display_name must be 1..={MAX_DISPLAY_NAME_LEN} chars")]
    BadDisplayName,
    /// `version` is zero.
    #[error("version must be >= 1")]
    BadVersion,
    /// More aliases than the bound allows.
    #[error("aliases carries {0}, at most {MAX_ALIASES} are allowed")]
    TooManyAliases(usize),
    /// An alias is not a slug, repeats, or names the topic itself.
    #[error("alias {alias:?}: {why}")]
    BadAlias {
        /// The offending alias.
        alias: String,
        /// What is wrong.
        why: &'static str,
    },
    /// `runner_id` is not `[a-z0-9][a-z0-9_-]{1,63}`.
    #[error("runner_id {0:?} must match [a-z0-9][a-z0-9_-]{{1,63}}")]
    BadRunnerId(String),
    /// A pin is not `sha256:<64 hex>`.
    #[error("{field} {got:?} is not {DIGEST_PREFIX}<64 lowercase hex>")]
    BadDigest {
        /// Which pin (`pin_rlm`, `pin_experiment`, `pack_digest`).
        field: &'static str,
        /// What the bundle said.
        got: String,
    },
    /// An in-guest runner with no pack to run.
    #[error(
        "runner_id {runner_id:?} names an in-guest runner, so pack_digest is required \
         (sha256:<64 hex> of the pack tar staged on the KVM host; never invented)"
    )]
    RunnerWithoutPack {
        /// The runner the bundle named.
        runner_id: String,
    },
    /// A pack nothing runs.
    #[error("pack_digest is set but runner_id is absent: a pack no runner reads is dead weight")]
    PackWithoutRunner,
    /// `n_concurrent` is zero.
    #[error("n_concurrent must be >= 1")]
    BadConcurrency,
    /// `version` / `n_concurrent` does not fit the row's `INTEGER` column.
    #[error("{field} {got} does not fit the topic row (max {MAX_INT_COLUMN}); refused rather than clamped")]
    IntColumnOverflow {
        /// Which field.
        field: &'static str,
        /// What the bundle said.
        got: u32,
    },
    /// `sealed_custom_value` is not finite.
    #[error("sealed_custom_value {0} is not finite; a baseline must be a measured number")]
    NonFiniteSealedValue(f64),
    /// `config` is not a JSON object.
    #[error("config must be a JSON object, got {0}")]
    ConfigNotObject(&'static str),
    /// `config` is larger than the bound.
    #[error("config is {0} bytes of canonical JSON, at most {MAX_CONFIG_BYTES} are allowed")]
    ConfigTooLarge(usize),
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

/// One topic install bundle, as written by an operator.
///
/// Required keys are not defaulted, so a missing `topic_id` is a parse error
/// naming the field rather than an empty string that fails later. Optional
/// keys default to the fail-closed reading: no aliases, no runner, no pins,
/// one concurrent job, no sealed baseline, empty config.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopicInstallBundle {
    /// Must equal [`BUNDLE_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Topic slug (`[a-z0-9][a-z0-9-]{1,62}`). The Arch default for the first
    /// topic is `tb4`.
    pub topic_id: String,
    /// Human label for operator output.
    pub display_name: String,
    /// Monotonic install version for this topic (a re-sign is a new version).
    pub version: u32,
    /// Install target this bundle was written for.
    pub environment: InstallEnvironment,
    /// Extra slugs the topic answers to. The Arch default is `["tbench"]`
    /// for topic `tb4`, so old miner links resolve to one row.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// In-guest runner id, if the topic selects one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_id: Option<String>,
    /// `sha256:<hex>` of the RLM VM image, or absent when not pinned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin_rlm: Option<String>,
    /// `sha256:<hex>` of the experiment guest image, or absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin_experiment: Option<String>,
    /// `sha256:<hex>` of the experiment pack tar, or absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack_digest: Option<String>,
    /// Jobs of this topic that may run at once.
    #[serde(default = "default_n_concurrent")]
    pub n_concurrent: u32,
    /// The sealed baseline primary, once measured. Absent until the seal path
    /// has a number; a topic with no sealed value cannot be enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sealed_custom_value: Option<f64>,
    /// Opaque per-topic operator config. Stored verbatim; this crate only
    /// checks that it is a bounded JSON object.
    #[serde(default = "default_config")]
    pub config: Value,
}

fn default_n_concurrent() -> u32 {
    1
}

fn default_config() -> Value {
    Value::Object(serde_json::Map::new())
}

impl Default for TopicInstallBundle {
    fn default() -> Self {
        Self {
            schema_version: BUNDLE_SCHEMA_VERSION,
            topic_id: String::new(),
            display_name: String::new(),
            version: 1,
            environment: InstallEnvironment::Staging,
            aliases: Vec::new(),
            runner_id: None,
            pin_rlm: None,
            pin_experiment: None,
            pack_digest: None,
            n_concurrent: default_n_concurrent(),
            sealed_custom_value: None,
            config: default_config(),
        }
    }
}

/// The resolved install: what a `--dry-run` prints and what a real install
/// writes. `enabled` is always `false` — installing a topic never opens it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TopicInstallPlan {
    /// Topic slug (the row's primary key).
    pub topic_id: String,
    /// Human label.
    pub display_name: String,
    /// Install version.
    pub version: u32,
    /// Install target.
    pub environment: InstallEnvironment,
    /// Extra slugs, sorted and de-duplicated.
    pub aliases: Vec<String>,
    /// In-guest runner id, empty when the topic selects none.
    pub runner_id: String,
    /// RLM image pin, empty when unpinned.
    pub pin_rlm: String,
    /// Experiment guest image pin, empty when unpinned.
    pub pin_experiment: String,
    /// Experiment pack digest, empty when absent.
    pub pack_digest: String,
    /// Concurrency bound.
    pub n_concurrent: u32,
    /// Sealed baseline primary, when the bundle carries one.
    pub sealed_custom_value: Option<f64>,
    /// Bundle schema version.
    pub schema_version: u32,
    /// The opaque per-topic operator config, verbatim.
    pub config: Value,
    /// `sha256:<hex>` over the canonical bundle.
    pub bundle_digest: String,
    /// Always `false` on install. A topic is enabled by an operator action
    /// that P0 does not implement.
    pub enabled: bool,
}

fn is_digest(s: &str) -> bool {
    s.strip_prefix(DIGEST_PREFIX).is_some_and(is_lower_hex64)
}

/// Exactly 64 **lowercase** hex characters, with no surrounding whitespace.
///
/// Deliberately stricter than `proof_canon::is_hex64`, which trims and accepts
/// uppercase: a pin is stored here verbatim and the row's `CHECK` is
/// `^sha256:[0-9a-f]{64}$`, so accepting `sha256:AB…` or `sha256: ab… ` would
/// let a bundle validate and dry-run and then fail on a real install. One
/// spelling of a digest, checked the same way in both places.
fn is_lower_hex64(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl TopicInstallBundle {
    /// Parse a bundle body (JSON only).
    ///
    /// Unknown keys are refused here: a binding this build cannot name is a
    /// binding it cannot enforce.
    pub fn from_json(body: &str) -> Result<Self, BundleError> {
        serde_json::from_str(body).map_err(|e| BundleError::Parse(e.to_string()))
    }

    /// Shape checks: ids, pins, cross-field rules, config bounds.
    ///
    /// Every value checked here is stored as given — nothing is normalised,
    /// substituted, or defaulted into existence.
    pub fn validate(&self) -> Result<(), BundleError> {
        if self.schema_version != BUNDLE_SCHEMA_VERSION {
            return Err(BundleError::WrongSchema {
                got: self.schema_version,
                want: BUNDLE_SCHEMA_VERSION,
            });
        }
        if !proof_canon::is_slug(&self.topic_id) {
            return Err(BundleError::BadTopicId(self.topic_id.clone()));
        }
        let name = self.display_name.trim();
        if name.is_empty() || name.chars().count() > MAX_DISPLAY_NAME_LEN {
            return Err(BundleError::BadDisplayName);
        }
        if self.version == 0 {
            return Err(BundleError::BadVersion);
        }
        if self.aliases.len() > MAX_ALIASES {
            return Err(BundleError::TooManyAliases(self.aliases.len()));
        }
        for alias in &self.aliases {
            let why = if !proof_canon::is_slug(alias) {
                "must match [a-z0-9][a-z0-9-]{1,62}"
            } else if alias == &self.topic_id {
                "an alias of the topic id itself is not an alias"
            } else if self.aliases.iter().filter(|a| *a == alias).count() > 1 {
                "duplicate alias"
            } else {
                continue;
            };
            return Err(BundleError::BadAlias {
                alias: alias.clone(),
                why,
            });
        }
        if let Some(id) = self.runner_id.as_deref() {
            if !proof_canon::is_custom_id(id) {
                return Err(BundleError::BadRunnerId(id.to_owned()));
            }
        }
        for (field, value) in [
            ("pin_rlm", self.pin_rlm.as_deref()),
            ("pin_experiment", self.pin_experiment.as_deref()),
            ("pack_digest", self.pack_digest.as_deref()),
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
        if self.runner_id.is_some() && self.pack_digest.is_none() {
            return Err(BundleError::RunnerWithoutPack {
                runner_id: self.runner_id.clone().unwrap_or_default(),
            });
        }
        if self.pack_digest.is_some() && self.runner_id.is_none() {
            return Err(BundleError::PackWithoutRunner);
        }
        if self.n_concurrent == 0 {
            return Err(BundleError::BadConcurrency);
        }
        for (field, value) in [
            ("version", self.version),
            ("n_concurrent", self.n_concurrent),
        ] {
            if value > MAX_INT_COLUMN {
                return Err(BundleError::IntColumnOverflow { field, got: value });
            }
        }
        if let Some(v) = self.sealed_custom_value {
            if !v.is_finite() {
                return Err(BundleError::NonFiniteSealedValue(v));
            }
        }
        match &self.config {
            Value::Object(_) => {}
            other => {
                return Err(BundleError::ConfigNotObject(json_kind(other)));
            }
        }
        // The bound is on the `config` object, measured on its own canonical
        // form: a large-but-legal bundle elsewhere must not be blamed on a
        // config that is well inside the limit.
        let config_bytes = proof_canon::canonical_json(&self.config).len();
        if config_bytes > MAX_CONFIG_BYTES {
            return Err(BundleError::ConfigTooLarge(config_bytes));
        }
        Ok(())
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
        self.validate()?;
        if self.environment != requested {
            return Err(BundleError::EnvironmentMismatch {
                bundle: self.environment,
                requested,
            });
        }
        let mut aliases = self.aliases.clone();
        aliases.sort_unstable();
        aliases.dedup();
        Ok(TopicInstallPlan {
            topic_id: self.topic_id.clone(),
            display_name: self.display_name.trim().to_owned(),
            version: self.version,
            environment: self.environment,
            aliases,
            runner_id: self.runner_id.clone().unwrap_or_default(),
            pin_rlm: self.pin_rlm.clone().unwrap_or_default(),
            pin_experiment: self.pin_experiment.clone().unwrap_or_default(),
            pack_digest: self.pack_digest.clone().unwrap_or_default(),
            n_concurrent: self.n_concurrent,
            sealed_custom_value: self.sealed_custom_value,
            schema_version: self.schema_version,
            config: self.config.clone(),
            bundle_digest: self.digest()?,
            enabled: false,
        })
    }
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEX: &str = "abababababababababababababababababababababababababababababababab";

    fn digest() -> String {
        format!("{DIGEST_PREFIX}{HEX}")
    }

    /// The Arch default for the first topic: slug `tb4`, alias `tbench`.
    fn tb4() -> TopicInstallBundle {
        TopicInstallBundle {
            topic_id: "tb4".into(),
            display_name: "Terminal-Bench 4".into(),
            environment: InstallEnvironment::Metal,
            aliases: vec!["tbench".into()],
            runner_id: Some("rlm_fc_in_guest_harbor".into()),
            pack_digest: Some(digest()),
            pin_rlm: Some(digest()),
            pin_experiment: Some(digest()),
            n_concurrent: 2,
            ..TopicInstallBundle::default()
        }
    }

    #[test]
    fn the_arch_default_topic_validates_and_plans_disabled() {
        let bundle = tb4();
        bundle.validate().expect("tb4 validates");
        let plan = bundle.plan(InstallEnvironment::Metal).expect("plan");
        assert_eq!(plan.topic_id, "tb4");
        assert_eq!(plan.aliases, ["tbench"]);
        assert_eq!(plan.environment, InstallEnvironment::Metal);
        assert!(!plan.enabled, "install never enables a topic");
        assert!(plan.bundle_digest.starts_with(DIGEST_PREFIX));
        assert_eq!(plan.bundle_digest.len(), DIGEST_PREFIX.len() + 64);
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
    fn unknown_keys_are_refused_at_parse() {
        let body = r#"{
            "schema_version": 1, "topic_id": "tb4", "display_name": "x",
            "version": 1, "environment": "metal", "task_slice": "tb4-first-15"
        }"#;
        let err = TopicInstallBundle::from_json(body).expect_err("unknown key");
        assert!(
            matches!(err, BundleError::Parse(ref m) if m.contains("task_slice")),
            "{err}"
        );
    }

    #[test]
    fn required_keys_are_named_when_absent() {
        for (body, missing) in [
            (
                r#"{"schema_version":1,"display_name":"x","version":1,"environment":"metal"}"#,
                "topic_id",
            ),
            (
                r#"{"schema_version":1,"topic_id":"tb4","version":1,"environment":"metal"}"#,
                "display_name",
            ),
            (
                r#"{"schema_version":1,"topic_id":"tb4","display_name":"x","environment":"metal"}"#,
                "version",
            ),
            (
                r#"{"schema_version":1,"topic_id":"tb4","display_name":"x","version":1}"#,
                "environment",
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
    fn ids_pins_and_bounds_are_checked() {
        let mut bundle = tb4();
        bundle.topic_id = "TB4".into();
        assert!(matches!(bundle.validate(), Err(BundleError::BadTopicId(_))));
        bundle = tb4();
        bundle.topic_id = "tbench_tb4".into();
        assert!(matches!(bundle.validate(), Err(BundleError::BadTopicId(_))));
        bundle = tb4();
        bundle.display_name = "  ".into();
        assert!(matches!(
            bundle.validate(),
            Err(BundleError::BadDisplayName)
        ));
        bundle = tb4();
        bundle.version = 0;
        assert!(matches!(bundle.validate(), Err(BundleError::BadVersion)));
        bundle = tb4();
        bundle.n_concurrent = 0;
        assert!(matches!(
            bundle.validate(),
            Err(BundleError::BadConcurrency)
        ));
        bundle = tb4();
        bundle.schema_version = 2;
        assert!(matches!(
            bundle.validate(),
            Err(BundleError::WrongSchema { got: 2, want: 1 })
        ));
    }

    #[test]
    fn a_digest_is_never_invented() {
        for bad in ["", "abc", HEX, "sha256:", "sha256:zz", "sha512:dead"] {
            let mut bundle = tb4();
            bundle.pin_rlm = Some(bad.into());
            assert!(
                matches!(
                    bundle.validate(),
                    Err(BundleError::BadDigest {
                        field: "pin_rlm",
                        ..
                    })
                ),
                "{bad:?} must be refused"
            );
        }
        // Absent is the only alternative to a well-formed digest.
        let mut unpinned = tb4();
        unpinned.pin_rlm = None;
        unpinned.pin_experiment = None;
        unpinned
            .validate()
            .expect("unpinned is legal, not defaulted");
    }

    #[test]
    fn a_runner_and_its_pack_travel_together() {
        let mut no_pack = tb4();
        no_pack.pack_digest = None;
        let err = no_pack.validate().expect_err("runner without pack");
        assert!(
            matches!(err, BundleError::RunnerWithoutPack { ref runner_id }
                if runner_id == "rlm_fc_in_guest_harbor"),
            "{err:?}"
        );
        assert!(err.to_string().contains("never invented"), "{err}");

        let mut orphan = tb4();
        orphan.runner_id = None;
        assert!(matches!(
            orphan.validate(),
            Err(BundleError::PackWithoutRunner)
        ));

        let mut bad_id = tb4();
        bad_id.runner_id = Some("Runner With Spaces".into());
        assert!(matches!(
            bad_id.validate(),
            Err(BundleError::BadRunnerId(_))
        ));

        // No runner, no pack: the harvest-family shape is legal.
        let mut harvest = tb4();
        harvest.runner_id = None;
        harvest.pack_digest = None;
        harvest.validate().expect("a topic may select no runner");
    }

    #[test]
    fn aliases_are_slugs_unique_and_never_the_topic_id() {
        let mut dup = tb4();
        dup.aliases = vec!["tbench".into(), "tbench".into()];
        assert!(matches!(
            dup.validate(),
            Err(BundleError::BadAlias {
                why: "duplicate alias",
                ..
            })
        ));

        let mut self_alias = tb4();
        self_alias.aliases = vec!["tb4".into()];
        assert!(matches!(
            self_alias.validate(),
            Err(BundleError::BadAlias {
                why: "an alias of the topic id itself is not an alias",
                ..
            })
        ));

        let mut malformed = tb4();
        malformed.aliases = vec!["TBench".into()];
        assert!(matches!(
            malformed.validate(),
            Err(BundleError::BadAlias { .. })
        ));

        let mut many = tb4();
        many.aliases = (0..=MAX_ALIASES).map(|i| format!("alias-{i}")).collect();
        assert!(matches!(
            many.validate(),
            Err(BundleError::TooManyAliases(9))
        ));

        // Order does not matter: the plan sorts and de-duplicates.
        let mut two = tb4();
        two.aliases = vec!["zeta".into(), "alpha".into()];
        assert_eq!(
            two.plan(InstallEnvironment::Metal).expect("plan").aliases,
            ["alpha", "zeta"]
        );
    }

    #[test]
    fn a_baseline_must_be_finite_and_config_must_be_a_bounded_object() {
        let mut nan = tb4();
        nan.sealed_custom_value = Some(f64::NAN);
        assert!(matches!(
            nan.validate(),
            Err(BundleError::NonFiniteSealedValue(_))
        ));
        let mut inf = tb4();
        inf.sealed_custom_value = Some(f64::INFINITY);
        assert!(matches!(
            inf.validate(),
            Err(BundleError::NonFiniteSealedValue(_))
        ));
        let mut sealed = tb4();
        sealed.sealed_custom_value = Some(0.42);
        sealed.validate().expect("a finite baseline is fine");

        let mut list = tb4();
        list.config = serde_json::json!([1, 2]);
        assert!(matches!(
            list.validate(),
            Err(BundleError::ConfigNotObject("an array"))
        ));
        let mut huge = tb4();
        huge.config = serde_json::json!({ "pad": "x".repeat(MAX_CONFIG_BYTES) });
        assert!(matches!(
            huge.validate(),
            Err(BundleError::ConfigTooLarge(_))
        ));
        let mut null = tb4();
        null.config = Value::Null;
        assert!(matches!(
            null.validate(),
            Err(BundleError::ConfigNotObject("null"))
        ));
    }

    #[test]
    fn defaults_are_the_fail_closed_reading() {
        let body = r#"{
            "schema_version": 1, "topic_id": "tb4", "display_name": "Terminal-Bench 4",
            "version": 1, "environment": "staging"
        }"#;
        let bundle = TopicInstallBundle::from_json(body).expect("parse");
        assert!(bundle.aliases.is_empty());
        assert!(bundle.runner_id.is_none());
        assert!(bundle.pin_rlm.is_none());
        assert!(bundle.pack_digest.is_none());
        assert_eq!(bundle.n_concurrent, 1);
        assert!(bundle.sealed_custom_value.is_none());
        assert_eq!(bundle.config, Value::Object(serde_json::Map::new()));
        bundle.validate().expect("defaults validate");
    }

    #[test]
    fn the_digest_ignores_formatting_and_key_order() {
        let compact = r#"{"schema_version":1,"topic_id":"tb4","display_name":"Terminal-Bench 4","version":1,"environment":"metal"}"#;
        let spaced = r#"{
            "environment": "metal",
            "version": 1,
            "display_name": "Terminal-Bench 4",
            "topic_id": "tb4",
            "schema_version": 1
        }"#;
        let a = TopicInstallBundle::from_json(compact).expect("a");
        let b = TopicInstallBundle::from_json(spaced).expect("b");
        assert_eq!(a.digest().expect("digest a"), b.digest().expect("digest b"));

        // Any real change is a different install, so a different digest.
        let mut changed = a.clone();
        changed.n_concurrent = 3;
        assert_ne!(
            a.digest().expect("a"),
            changed.digest().expect("changed"),
            "a changed bundle must not hash the same"
        );
        let mut renamed = a;
        renamed.topic_id = "tb5".into();
        assert_ne!(
            b.digest().expect("b"),
            renamed.digest().expect("renamed"),
            "the topic id is part of the identity"
        );
    }

    #[test]
    fn the_digest_is_stable_and_matches_a_pinned_vector() {
        // A literal vector: if the canonical form or the digest algorithm ever
        // drifts, this test fails rather than silently re-pinning every topic.
        let body = r#"{"schema_version":1,"topic_id":"tb4","display_name":"Terminal-Bench 4","version":1,"environment":"metal"}"#;
        let bundle = TopicInstallBundle::from_json(body).expect("parse");
        let canonical = bundle.canonical().expect("canonical");
        assert_eq!(
            canonical,
            r#"{"aliases":[],"config":{},"display_name":"Terminal-Bench 4","environment":"metal","n_concurrent":1,"schema_version":1,"topic_id":"tb4","version":1}"#
        );
        let digest = bundle.digest().expect("digest");
        assert_eq!(digest, format!("{DIGEST_PREFIX}{}", sha256_hex(&canonical)));
    }

    fn sha256_hex(s: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(s.as_bytes());
        hex::encode(h.finalize())
    }

    /// The row's `CHECK` is `^sha256:[0-9a-f]{64}$`, so anything this
    /// validator accepts has to be exactly that. Accepting an uppercase or
    /// padded pin would let a bundle validate and dry-run and then fail a real
    /// install — the operator would find out only on the host that matters.
    #[test]
    fn digest_pins_are_exactly_lowercase_and_unpadded() {
        let upper = HEX.to_ascii_uppercase();
        for bad in [
            format!("sha256:{upper}"),
            format!("sha256: {HEX}"),
            format!("sha256:{HEX} "),
            format!(" sha256:{HEX}"),
            format!("sha256:{}", &HEX[..63]),
            format!("sha256:{HEX}0"),
            format!("SHA256:{HEX}"),
        ] {
            for field in ["pin_rlm", "pin_experiment", "pack_digest"] {
                let mut bundle = tb4();
                match field {
                    "pin_rlm" => bundle.pin_rlm = Some(bad.clone()),
                    "pin_experiment" => bundle.pin_experiment = Some(bad.clone()),
                    _ => bundle.pack_digest = Some(bad.clone()),
                }
                assert!(
                    matches!(
                        bundle.validate(),
                        Err(BundleError::BadDigest { field: f, .. }) if f == field
                    ),
                    "{field}={bad:?} must be refused, not silently accepted"
                );
            }
        }
        // The canonical spelling still validates, so this is strictness
        // rather than a blanket rejection.
        tb4().validate().expect("lowercase hex validates");
        assert!(is_lower_hex64(HEX));
        assert!(!is_lower_hex64(&upper));
        assert!(!is_lower_hex64(&format!(" {HEX}")));
    }

    /// The row's columns are `INTEGER`. A value that would not fit is a
    /// reject, never a clamp: a clamped row would disagree with the
    /// digest-covered bundle the operator reviewed.
    #[test]
    fn numeric_columns_out_of_range_are_refused_not_clamped() {
        let mut big_version = tb4();
        big_version.version = MAX_INT_COLUMN + 1;
        let err = big_version.validate().expect_err("version overflow");
        assert!(
            matches!(
                err,
                BundleError::IntColumnOverflow {
                    field: "version",
                    got: _
                }
            ),
            "{err:?}"
        );
        assert!(
            err.to_string().contains("refused rather than clamped"),
            "{err}"
        );

        let mut big_concurrency = tb4();
        big_concurrency.n_concurrent = u32::MAX;
        assert!(
            matches!(
                big_concurrency.validate(),
                Err(BundleError::IntColumnOverflow {
                    field: "n_concurrent",
                    ..
                })
            ),
            "n_concurrent overflow"
        );

        // The boundary itself is legal.
        let mut at_limit = tb4();
        at_limit.version = MAX_INT_COLUMN;
        at_limit.n_concurrent = MAX_INT_COLUMN;
        at_limit.validate().expect("i32::MAX fits the column");
        assert_eq!(MAX_INT_COLUMN, i32::MAX as u32);
    }

    /// The advertised limit is on the `config` object. A legal bundle with
    /// long metadata must not be rejected for a small config, and the error
    /// must report the config's own size rather than the bundle's.
    #[test]
    fn the_config_bound_measures_the_config_not_the_bundle() {
        // A small config inside a large-but-legal bundle.
        let mut bundle = tb4();
        bundle.display_name = "x".repeat(MAX_DISPLAY_NAME_LEN);
        bundle.aliases = (0..MAX_ALIASES).map(|i| format!("alias-{i}")).collect();
        bundle.config = serde_json::json!({ "task_slice": "tb4-first-15" });
        bundle
            .validate()
            .expect("a small config in a large bundle is fine");

        // Over the limit is refused, and the number is the config's own size.
        let mut over = tb4();
        over.config = serde_json::json!({ "pad": "x".repeat(MAX_CONFIG_BYTES) });
        let err = over.validate().expect_err("oversized config");
        let BundleError::ConfigTooLarge(reported) = err else {
            panic!("expected ConfigTooLarge, got {err:?}");
        };
        let config_bytes = proof_canon::canonical_json(&over.config).len();
        assert_eq!(
            reported, config_bytes,
            "the error must report the config's size"
        );
        assert!(config_bytes > MAX_CONFIG_BYTES);

        // Exactly at the limit passes.
        let mut at_limit = tb4();
        let overhead = proof_canon::canonical_json(&serde_json::json!({ "pad": "" })).len();
        at_limit.config = serde_json::json!({ "pad": "x".repeat(MAX_CONFIG_BYTES - overhead) });
        assert_eq!(
            proof_canon::canonical_json(&at_limit.config).len(),
            MAX_CONFIG_BYTES
        );
        at_limit.validate().expect("exactly at the limit passes");
    }

    #[test]
    fn the_schema_key_list_matches_the_type() {
        // A full bundle with every key set: the serialized form must carry
        // exactly `BUNDLE_KEYS`, and each one must round-trip.
        let bundle = TopicInstallBundle {
            sealed_custom_value: Some(0.5),
            ..tb4()
        };
        let value = serde_json::to_value(&bundle).expect("serialize");
        let mut keys: Vec<String> = value.as_object().expect("object").keys().cloned().collect();
        keys.sort_unstable();
        assert_eq!(keys, BUNDLE_KEYS, "the schema key list drifted");
        for key in REQUIRED_BUNDLE_KEYS {
            assert!(keys.iter().any(|k| k == key), "{key} must be required");
        }
    }

    #[test]
    fn a_plan_is_serialisable_for_the_dry_run_json_output() {
        let plan = tb4().plan(InstallEnvironment::Metal).expect("plan");
        let body = serde_json::to_string(&plan).expect("json");
        assert!(body.contains(r#""topic_id":"tb4""#), "{body}");
        assert!(body.contains(r#""enabled":false"#), "{body}");
        assert!(body.contains(r#""environment":"metal""#), "{body}");
        let round: TopicInstallPlan = serde_json::from_str(&body).expect("round trip");
        assert_eq!(round, plan);
    }
}
