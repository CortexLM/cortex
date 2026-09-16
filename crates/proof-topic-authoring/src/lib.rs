//! What a topic's RLM **authors**, and the shape checks every part is held to.
//!
//! A Proof topic's behavior is not compiled into any binary and is not the
//! operator's to write. The topic's own RLM runs inside its topic VM and
//! authors the whole set:
//!
//! | Part | What it is | Where it lands |
//! |------|-----------|----------------|
//! | `rules` | the anti-cheat vector ticked before any paid inference | `proof_rule_version` (`source = 'rlm'`) |
//! | `migrations` | the topic's own SQL, applied under the deny-list | the shared database, inside the topic's namespace |
//! | `apis` | the routes the topic exposes for itself | `proof_topic_api` |
//! | `submission_format` | the wire shape it accepts | recorded as a canonical digest |
//! | `pin_policy` | how it **tightens** the global pin | recorded as a canonical digest |
//!
//! This crate is the one place those five parts are *described*: their shapes,
//! their bounds, their canonical digests, and the tightening rule that makes a
//! policy a policy (a topic may tighten a floor, never loosen it). It holds no
//! database, no VM, and no challenge content — it is pure data and text
//! analysis, so the guest that validates what its RLM emitted and the control
//! plane that applies it agree by construction rather than by convention.
//!
//! # Why the checks live here
//!
//! Two very different processes have to answer the same question — "is this a
//! well-formed set for this topic?" — and they must not answer it differently:
//!
//! - the **guest**, before it hands an RLM's answer back over the wire, so a
//!   malformed or denied set never becomes a job output; and
//! - the **install**, before it applies a migration, registers a route, or
//!   journals a binding, so a set that would leave the database in a state the
//!   deny-list forbids never runs.
//!
//! The guest cannot link the install (it has no database) and the install
//! cannot link the guest. Both can link this.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::must_use_candidate
)]

pub mod handler;
pub mod section;

pub use handler::{
    allowed_list, bound_runner, check_handler, resolve_handler, Handler, HandlerError,
    ALLOWED_HANDLERS,
};
pub use section::{read_apis, read_migrations, read_rules, read_section, SectionPlan, READ_KEYS};

use proof_task::{ChecklistRule, ProofPin, TopicDocument};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Only accepted `schema_version` of an authored set.
pub const AUTHORING_SCHEMA: u32 = 1;

/// Most migrations one topic may author.
pub const MAX_MIGRATIONS: usize = 64;

/// Longest one migration's SQL may be, in bytes.
pub const MAX_MIGRATION_SQL_BYTES: usize = 256 * 1024;

/// Most routes one topic may register.
pub const MAX_APIS: usize = 64;

/// Longest route summary, in characters.
pub const MAX_API_SUMMARY_CHARS: usize = 256;

/// Longest `eval_image_digest` / `gpu_class` pin a policy may carry.
pub const MAX_PIN_STRING_CHARS: usize = 256;

/// Path prefixes inside a topic's own namespace that are **not a topic's to
/// claim**: the challenge's operator surface.
///
/// A topic route is served under the topic's prefix
/// (`/challenge/{topic_id}/{path}`), so a stored `v1/admin/…` would answer at
/// `/challenge/{topic_id}/v1/admin/…` — a path a reader cannot tell apart
/// from the challenge's own admin surface, which is master-local. Both the
/// authoring check and the install refuse one, and the mux refuses to resolve
/// a row that is already in the table (a row written before this rule
/// existed).
pub const RESERVED_API_PREFIXES: [&str; 1] = ["v1/admin"];

/// One SQL migration the topic's RLM authored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthoredMigration {
    /// Topic-facing name, an id (`[a-z0-9][a-z0-9_-]{1,63}`).
    pub name: String,
    /// The SQL. Applied under the deny-list; never logged in full.
    pub sql: String,
}

/// One route the topic's RLM authored for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthoredApi {
    /// Path relative to the topic's own prefix: no leading `/`, no `..`.
    pub path: String,
    /// Upper-cased HTTP method, or `*`.
    pub method: String,
    /// What the route does, in the topic's words.
    #[serde(default)]
    pub summary: String,
}

/// The migration shape under the name the install and the store read it by.
pub type Migration = AuthoredMigration;

/// The route shape under the name the install and the mux read it by.
pub type ApiRoute = AuthoredApi;

/// A relative path of plain segments: no leading `/`, no `.` / `..`, no empty
/// segment, no control characters, no backslash.
///
/// A free function as well as [`TopicAuthoring::is_relative_api_path`],
/// because the readers that hold one part rather than a whole set (the mux,
/// the install's route resolver) want the predicate without a set in hand.
#[must_use]
pub fn is_relative_api_path(p: &str) -> bool {
    TopicAuthoring::is_relative_api_path(p)
}

/// A method a topic may claim.
#[must_use]
pub fn is_api_method(m: &str) -> bool {
    TopicAuthoring::is_api_method(m)
}

/// Whether `p` is inside a [`RESERVED_API_PREFIXES`] namespace.
#[must_use]
pub fn is_reserved_api_path(p: &str) -> bool {
    TopicAuthoring::is_reserved_api_path(p)
}

/// How a topic **tightens** the global pin. Every field is optional: an
/// absent knob means "this topic tightens nothing here".
///
/// The rule is one-directional and enforced field by field: a policy may make
/// the topic *stricter* than the global pin, never looser. A topic that could
/// lower a floor would be a topic that scores something the network did not
/// agree to, which is why [`PinPolicy::tightens`] refuses by name rather than
/// clamping silently.
///
/// Three fields are **equalities**, not tightenings: `holdout_size`,
/// `eval_image_digest`, and `gpu_class` describe what every topic is measured
/// against, so a policy that names them must name exactly the pin's value. A
/// topic may not score on another image or another machine class.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PinPolicy {
    /// Floor on the topic's absolute NLL epsilon (may only be raised).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub epsilon_nll_min: Option<f64>,
    /// Floor on a throughput topic's relative win (may only be raised).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub epsilon_throughput_rel_min: Option<f64>,
    /// Floor on a topic's per-split NLL regression tolerance (raised only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub epsilon_topic_max_regress_min: Option<f64>,
    /// Proof deadline the topic accepts (may only be lowered, never zero).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_proof_deadline_s: Option<u64>,
    /// Largest FLOP budget the topic accepts (may only be lowered).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flops_budget_max: Option<u64>,
    /// Holdout records per topic. An equality: it must be the pin's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub holdout_size: Option<usize>,
    /// Eval image the topic scores on. An equality: it must be the pin's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eval_image_digest: Option<String>,
    /// Machine class the topic scores on. An equality: it must be the pin's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_class: Option<String>,
}

impl PinPolicy {
    /// The policy that tightens nothing.
    pub fn none() -> Self {
        Self::default()
    }

    /// Whether every knob is absent.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Field-level shape: finite floors, non-zero deadlines and budgets,
    /// bounded strings. Says nothing about the pin — that is
    /// [`Self::tightens`].
    pub fn validate_shape(&self) -> Result<(), AuthoringError> {
        let floors: [(&str, Option<f64>); 3] = [
            ("epsilon_nll_min", self.epsilon_nll_min),
            (
                "epsilon_throughput_rel_min",
                self.epsilon_throughput_rel_min,
            ),
            (
                "epsilon_topic_max_regress_min",
                self.epsilon_topic_max_regress_min,
            ),
        ];
        for (field, value) in floors {
            if let Some(v) = value {
                if !v.is_finite() || v <= 0.0 || v > 1.0 {
                    return Err(AuthoringError::PinPolicy {
                        field,
                        why: "must be a finite fraction in (0, 1]".into(),
                    });
                }
            }
        }
        if self.max_proof_deadline_s == Some(0) {
            return Err(AuthoringError::PinPolicy {
                field: "max_proof_deadline_s",
                why: "must be at least 1 second; remove the knob to accept the pin's ceiling"
                    .into(),
            });
        }
        if self.flops_budget_max == Some(0) {
            return Err(AuthoringError::PinPolicy {
                field: "flops_budget_max",
                why: "must be at least 1; remove the knob to accept the pin's maximum".into(),
            });
        }
        if self.holdout_size == Some(0) {
            return Err(AuthoringError::PinPolicy {
                field: "holdout_size",
                why: "must be at least 1".into(),
            });
        }
        for (field, value) in [
            ("eval_image_digest", self.eval_image_digest.as_deref()),
            ("gpu_class", self.gpu_class.as_deref()),
        ] {
            if let Some(s) = value {
                if s.trim().is_empty() || s.chars().count() > MAX_PIN_STRING_CHARS {
                    return Err(AuthoringError::PinPolicy {
                        field,
                        why: format!("must be 1..={MAX_PIN_STRING_CHARS} chars"),
                    });
                }
            }
        }
        Ok(())
    }

    /// Refuse any knob that is **looser** than the global pin, and any
    /// equality that disagrees with it.
    pub fn tightens(&self, pin: &ProofPin) -> Result<(), AuthoringError> {
        self.validate_shape()?;
        let floors: [(&str, Option<f64>, f64); 3] = [
            ("epsilon_nll_min", self.epsilon_nll_min, pin.epsilon_nll_min),
            (
                "epsilon_throughput_rel_min",
                self.epsilon_throughput_rel_min,
                pin.epsilon_throughput_rel_min,
            ),
            (
                "epsilon_topic_max_regress_min",
                self.epsilon_topic_max_regress_min,
                pin.epsilon_topic_max_regress_min,
            ),
        ];
        for (field, asked, floor) in floors {
            if let Some(v) = asked {
                if v < floor {
                    return Err(AuthoringError::LoosenedFloor {
                        field,
                        got: v,
                        floor,
                    });
                }
            }
        }
        if let Some(asked) = self.max_proof_deadline_s {
            if asked > pin.max_proof_deadline_s_ceiling {
                return Err(AuthoringError::LoosenedCeiling {
                    field: "max_proof_deadline_s",
                    got: asked,
                    ceiling: pin.max_proof_deadline_s_ceiling,
                });
            }
        }
        if let Some(asked) = self.flops_budget_max {
            if asked > pin.flops_budget_max {
                return Err(AuthoringError::LoosenedCeiling {
                    field: "flops_budget_max",
                    got: asked,
                    ceiling: pin.flops_budget_max,
                });
            }
        }
        let equals: [(&str, Option<String>, String); 3] = [
            (
                "eval_image_digest",
                self.eval_image_digest.clone(),
                pin.eval_image_digest.clone(),
            ),
            ("gpu_class", self.gpu_class.clone(), pin.gpu_class.clone()),
            (
                "holdout_size",
                self.holdout_size.map(|n| n.to_string()),
                pin.holdout_size.to_string(),
            ),
        ];
        for (field, asked, want) in equals {
            if let Some(asked) = asked {
                if asked != want {
                    return Err(AuthoringError::PinEquality {
                        field,
                        got: asked,
                        want,
                    });
                }
            }
        }
        Ok(())
    }

    /// Refuse a policy looser than the **signed document's own** knobs.
    ///
    /// The document is what miners are scored against, so a policy that
    /// accepted a *wider* range than the document declares would be a promise
    /// the host does not keep. This is the check a guest can run with only the
    /// job's own topic in hand (it has no pin).
    pub fn tightens_document(&self, doc: &TopicDocument) -> Result<(), AuthoringError> {
        self.validate_shape()?;
        let floors: [(&str, Option<f64>, f64); 3] = [
            ("epsilon_nll_min", self.epsilon_nll_min, doc.epsilon_nll),
            (
                "epsilon_throughput_rel_min",
                self.epsilon_throughput_rel_min,
                doc.metric.epsilon_rel,
            ),
            (
                "epsilon_topic_max_regress_min",
                self.epsilon_topic_max_regress_min,
                doc.epsilon_topic_max_regress,
            ),
        ];
        for (field, asked, document) in floors {
            if let Some(v) = asked {
                if v < document {
                    return Err(AuthoringError::LoosenedFloor {
                        field,
                        got: v,
                        floor: document,
                    });
                }
            }
        }
        if let Some(asked) = self.max_proof_deadline_s {
            // A document that declares no deadline accepts the pin's ceiling,
            // which the guest cannot read — so a policy that tightens *below*
            // it is accepted here and checked against the ceiling by the
            // control plane, which has the pin. A document that declares one
            // is the tighter bound, and the policy may not exceed it.
            if let Some(document) = doc.eval_executor.max_proof_deadline_s {
                if asked > document {
                    return Err(AuthoringError::LoosenedCeiling {
                        field: "max_proof_deadline_s",
                        got: asked,
                        ceiling: document,
                    });
                }
            }
        }
        if let Some(asked) = self.flops_budget_max {
            if asked > doc.flops_budget {
                return Err(AuthoringError::LoosenedCeiling {
                    field: "flops_budget_max",
                    got: asked,
                    ceiling: doc.flops_budget,
                });
            }
        }
        if let Some(asked) = self.holdout_size {
            if asked != doc.holdout_size {
                return Err(AuthoringError::PinEquality {
                    field: "holdout_size",
                    got: asked.to_string(),
                    want: doc.holdout_size.to_string(),
                });
            }
        }
        Ok(())
    }
}

/// The five parts a topic's RLM authors, as one document.
///
/// `deny_unknown_fields` is deliberate: a key this build does not read is a
/// part nothing applies, so an RLM that wrote one gets a refusal naming it
/// rather than a silent drop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopicAuthoring {
    /// Must equal [`AUTHORING_SCHEMA`].
    pub schema_version: u32,
    /// Topic the set is for. The host refuses a mismatch.
    pub topic_id: String,
    /// The anti-cheat vector.
    pub rules: Vec<ChecklistRule>,
    /// The topic's SQL, in apply order.
    pub migrations: Vec<AuthoredMigration>,
    /// The routes the topic exposes.
    pub apis: Vec<AuthoredApi>,
    /// The submission shape the topic accepts. Recorded, never interpreted.
    pub submission_format: Value,
    /// How the topic tightens the global pin.
    pub pin_policy: PinPolicy,
}

/// Why a set or a part of it was refused.
///
/// One error type for the whole crate: the RLM's own answer, the install's
/// reader, and the handler allow-list all refuse in these terms, so a refusal
/// reads the same wherever it happened.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AuthoringError {
    /// The set is for another topic.
    #[error("authored set is for topic {got:?}, the job is for {want:?}")]
    WrongTopic {
        /// What the set says.
        got: String,
        /// What the job is bound to.
        want: String,
    },
    /// Schema drift.
    #[error("authored set schema_version {got}, this build reads {AUTHORING_SCHEMA}")]
    WrongSchema {
        /// What the set said.
        got: u32,
    },
    /// A part is missing: the RLM authored an incomplete set.
    #[error(
        "the RLM authored no {part}: the set is incomplete, so nothing is installed — a topic's \
         behavior is authored by its own RLM (rules, migrations, apis, submission_format, \
         pin_policy), and an install refuses rather than filling the gap with the operator's \
         bundle"
    )]
    MissingPart {
        /// Which of the five parts.
        part: &'static str,
    },
    /// A part of the set is malformed or carries an unknown key.
    #[error("rlm.{part}: {why}")]
    Section {
        /// Which part (`migrations[0]`, `apis`, `rules`, …).
        part: String,
        /// What is wrong.
        why: String,
    },
    /// The rule vector was refused.
    #[error("rules: {0}")]
    Rules(String),
    /// A migration was refused by the shape check.
    #[error("migrations[{index}] ({name:?}): {why}")]
    Migration {
        /// Ordinal in the set.
        index: usize,
        /// The migration's name.
        name: String,
        /// What is wrong.
        why: String,
    },
    /// A migration reached outside the topic's namespace.
    #[error("migrations[{index}] ({name:?}): {why}")]
    MigrationDenied {
        /// Ordinal in the set.
        index: usize,
        /// The migration's name.
        name: String,
        /// The deny-list's reason, naming the statement and the object.
        why: String,
    },
    /// A route was refused.
    #[error("apis[{index}]: {why}")]
    Api {
        /// Ordinal in the set.
        index: usize,
        /// What is wrong.
        why: String,
    },
    /// The submission format is not an object.
    #[error("submission_format: must be a non-empty object, got {got}")]
    SubmissionFormat {
        /// The JSON kind that arrived.
        got: String,
    },
    /// A pin-policy knob is malformed.
    #[error("pin_policy.{field}: {why}")]
    PinPolicy {
        /// Which knob.
        field: &'static str,
        /// What is wrong.
        why: String,
    },
    /// A pin-policy floor is looser than the pin's.
    #[error("pin_policy.{field} = {got} loosens the floor {floor}")]
    LoosenedFloor {
        /// Which knob.
        field: &'static str,
        /// What the policy asked for.
        got: f64,
        /// The floor it may not go below.
        floor: f64,
    },
    /// A pin-policy ceiling is looser than the pin's.
    #[error("pin_policy.{field} = {got} loosens the ceiling {ceiling}")]
    LoosenedCeiling {
        /// Which knob.
        field: &'static str,
        /// What the policy asked for.
        got: u64,
        /// The ceiling it may not go above.
        ceiling: u64,
    },
    /// A pin-policy equality disagrees with the pin's.
    #[error("pin_policy.{field} = {got:?} must equal the pin's {want:?}")]
    PinEquality {
        /// Which knob.
        field: &'static str,
        /// What the policy asked for.
        got: String,
        /// What the pin says.
        want: String,
    },
    /// The set is larger than the bounds allow.
    #[error("authored set carries {count} {part}, at most {max} are applied")]
    TooMany {
        /// Which part.
        part: &'static str,
        /// How many arrived.
        count: usize,
        /// The bound.
        max: usize,
    },
    /// The set is not serialisable as an install section.
    #[error("authored set cannot be handed over: {0}")]
    Encode(String),
}

/// The part-level refusal an install's reader produces.
///
/// A distinct type from [`AuthoringError`] because it names a part and a
/// reason and nothing else: the install maps it into its own error, the guest
/// into its job failure, and neither has to carry the other's vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("rlm.{part}: {why}")]
pub struct SectionError {
    /// Which part (`migrations[0]`, `apis`, `rules`, …).
    pub part: String,
    /// What is wrong.
    pub why: String,
}

/// The five parts, by name, in the order they are reported.
pub const PARTS: [&str; 5] = [
    "rules",
    "migrations",
    "apis",
    "submission_format",
    "pin_policy",
];

/// A JSON value's kind, for an error that says what arrived.
fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

impl TopicAuthoring {
    /// A relative path of plain segments: no leading `/`, no `.` / `..`, no
    /// empty segment, no control characters, no backslash.
    pub fn is_relative_api_path(p: &str) -> bool {
        let p = p.trim();
        !p.is_empty()
            && p.len() <= 512
            && !p.starts_with('/')
            && !p.ends_with('/')
            && !p
                .chars()
                .any(|c| c.is_control() || c == '\\' || c == '?' || c == '#')
            && p.split('/')
                .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
            && p.split('/').all(|seg| {
                seg.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '~' | '-'))
            })
    }

    /// A method a topic may claim.
    pub fn is_api_method(m: &str) -> bool {
        matches!(m, "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "*")
    }

    /// Whether `p` is inside a [`RESERVED_API_PREFIXES`] namespace.
    ///
    /// Segment-aware: `v1/admin` and `v1/admin/…` are reserved,
    /// `v1/administrator` is not.
    pub fn is_reserved_api_path(p: &str) -> bool {
        let p = p.trim();
        RESERVED_API_PREFIXES.iter().any(|prefix| {
            p == *prefix
                || p.strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
    }

    /// Which of the five parts the set does **not** carry.
    ///
    /// Empty means the set is complete. This is the read the fail-closed gate
    /// uses, so the refusal names the missing part rather than saying "invalid".
    ///
    /// Four parts are checked for content here. `pin_policy` is not: its
    /// **presence** is structural (the key is required, so a set without it
    /// does not parse at all), while an empty policy is a legitimate answer —
    /// it is the RLM saying "this topic tightens nothing", which is what every
    /// topic did before the part existed. What the gate holds for that part is
    /// the tightening rule ([`PinPolicy::tightens`]), not a non-emptiness
    /// check that would demand a knob the RLM has no reason to set.
    pub fn missing_parts(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.rules.is_empty() {
            out.push("rules");
        }
        if self.migrations.is_empty() {
            out.push("migrations");
        }
        if self.apis.is_empty() {
            out.push("apis");
        }
        // A submission format that is an object with no keys is not a format:
        // nothing describes what the topic accepts, so it counts as missing.
        let format_empty = self
            .submission_format
            .as_object()
            .is_some_and(serde_json::Map::is_empty);
        if !self.submission_format.is_object() || format_empty {
            out.push("submission_format");
        }
        out
    }

    /// Whether every part is present.
    pub fn is_complete(&self) -> bool {
        self.missing_parts().is_empty()
    }

    /// Refuse the first missing part, naming it.
    pub fn require_complete(&self) -> Result<(), AuthoringError> {
        match self.missing_parts().first() {
            Some(part) => Err(AuthoringError::MissingPart { part }),
            None => Ok(()),
        }
    }

    /// Shape-check every part, refuse the first problem, and require the set
    /// to be complete.
    ///
    /// `topic_id` is the topic the job was bound to, so a set an RLM wrote for
    /// another topic is refused rather than applied.
    pub fn validate(&self, topic_id: &str) -> Result<(), AuthoringError> {
        if self.schema_version != AUTHORING_SCHEMA {
            return Err(AuthoringError::WrongSchema {
                got: self.schema_version,
            });
        }
        if self.topic_id.trim() != topic_id.trim() {
            return Err(AuthoringError::WrongTopic {
                got: self.topic_id.clone(),
                want: topic_id.to_owned(),
            });
        }
        // A part that *carried* the wrong kind is a different diagnosis from a
        // part that carried nothing, and the two refusals say so: "must be an
        // object, got an array" sends an author to the value, "authored no
        // submission_format" sends them to the missing part.
        if !self.submission_format.is_null() && !self.submission_format.is_object() {
            return Err(AuthoringError::SubmissionFormat {
                got: kind(&self.submission_format).to_owned(),
            });
        }
        self.require_complete()?;
        proof_canon::validate_rules(&self.rules)
            .map_err(|e| AuthoringError::Rules(format!("{}: {}", e.field, e.why)))?;
        self.check_migrations(topic_id)?;
        self.check_apis()?;
        if !self.submission_format.is_object() {
            return Err(AuthoringError::SubmissionFormat {
                got: kind(&self.submission_format).to_owned(),
            });
        }
        self.pin_policy.validate_shape()?;
        Ok(())
    }

    /// Shape-check the migrations, including the deny-list.
    ///
    /// The deny-list runs here as well as at install time, and that is
    /// deliberate: this is where the RLM's **own answer** arrives, so a
    /// migration that would be refused when applied never becomes a job output
    /// at all. The install runs the same check again because it cannot know
    /// who validated the set it was handed.
    fn check_migrations(&self, topic_id: &str) -> Result<(), AuthoringError> {
        if self.migrations.len() > MAX_MIGRATIONS {
            return Err(AuthoringError::TooMany {
                part: "migrations",
                count: self.migrations.len(),
                max: MAX_MIGRATIONS,
            });
        }
        for (index, m) in self.migrations.iter().enumerate() {
            let bad = |why: String| AuthoringError::Migration {
                index,
                name: m.name.clone(),
                why,
            };
            if !proof_canon::is_custom_id(&m.name) {
                return Err(bad("name is not an id ([a-z0-9][a-z0-9_-]{1,63})".into()));
            }
            if m.sql.trim().is_empty() {
                return Err(bad("sql is empty; remove the migration instead".into()));
            }
            if m.sql.len() > MAX_MIGRATION_SQL_BYTES {
                return Err(bad(format!(
                    "sql is {} bytes, at most {MAX_MIGRATION_SQL_BYTES} are applied",
                    m.sql.len()
                )));
            }
            proof_topic_sql_guard::check_migration(&m.sql, topic_id).map_err(|e| {
                AuthoringError::MigrationDenied {
                    index,
                    name: m.name.clone(),
                    why: e.to_string(),
                }
            })?;
        }
        Ok(())
    }

    /// Shape-check the routes: relative to the topic's prefix, never inside
    /// the challenge's admin namespace, a method the mux can answer.
    fn check_apis(&self) -> Result<(), AuthoringError> {
        if self.apis.len() > MAX_APIS {
            return Err(AuthoringError::TooMany {
                part: "apis",
                count: self.apis.len(),
                max: MAX_APIS,
            });
        }
        for (index, a) in self.apis.iter().enumerate() {
            let bad = |why: String| AuthoringError::Api { index, why };
            if !Self::is_relative_api_path(&a.path) {
                return Err(bad(format!(
                    "path {:?} must be a relative path of plain segments (no leading '/', no \
                     '..', no empty segment): a topic's routes live under its own prefix, and the \
                     prefix is the control plane's to set",
                    a.path
                )));
            }
            if Self::is_reserved_api_path(&a.path) {
                return Err(bad(format!(
                    "path {:?} is inside the challenge's admin namespace ({}), which is not a \
                     topic's to claim. Register a different path.",
                    a.path,
                    RESERVED_API_PREFIXES.join(", ")
                )));
            }
            if !Self::is_api_method(&a.method.trim().to_ascii_uppercase()) {
                return Err(bad(format!(
                    "method {:?} must be one of GET, POST, PUT, PATCH, DELETE, *",
                    a.method
                )));
            }
            if a.summary.chars().count() > MAX_API_SUMMARY_CHARS {
                return Err(bad(format!(
                    "summary is longer than {MAX_API_SUMMARY_CHARS} chars"
                )));
            }
        }
        Ok(())
    }

    /// Shape-check **and** hold the pin policy to the global pin.
    ///
    /// The control plane runs this (it has the pin); the guest runs
    /// [`Self::validate`] plus [`PinPolicy::tightens_document`], because the
    /// job carries the topic but not the pin.
    pub fn validate_against_pin(
        &self,
        topic_id: &str,
        pin: &ProofPin,
    ) -> Result<(), AuthoringError> {
        self.validate(topic_id)?;
        self.pin_policy.tightens(pin)
    }

    /// The set as the **install section** the installer reads.
    ///
    /// One shape, two sources: an operator's bundle and an RLM's answer are
    /// handed over identically, so the install applies whichever it was given
    /// through exactly the same gates. `handler` is absent — a handler is the
    /// *operator's* run backend, not something a topic's RLM chooses.
    pub fn as_section(&self) -> Result<String, AuthoringError> {
        let value = serde_json::json!({
            "rules": self.rules,
            "migrations": self.migrations,
            "apis": self.apis,
            "submission_format": self.submission_format,
            "pin_policy": self.pin_policy,
        });
        serde_json::to_string(&value).map_err(|e| AuthoringError::Encode(e.to_string()))
    }

    /// SHA-256 of one part's canonical JSON, `sha256:<hex>`.
    pub fn part_digest(&self, part: &str) -> String {
        let value = match part {
            "rules" => serde_json::to_value(&self.rules),
            "migrations" => serde_json::to_value(&self.migrations),
            "apis" => serde_json::to_value(&self.apis),
            "submission_format" => Ok(self.submission_format.clone()),
            "pin_policy" => serde_json::to_value(&self.pin_policy),
            _ => Ok(Value::Null),
        }
        .unwrap_or(Value::Null);
        digest_of(&value)
    }

    /// SHA-256 of the whole set's canonical JSON.
    pub fn digest(&self) -> String {
        domain_digest(b"proof-topic-authoring-v1", self)
    }

    /// The journal entry: per-part digests, the whole-set digest, and the
    /// provenance word for every part.
    ///
    /// This is what makes "the RLM authored this topic" a fact the journal can
    /// prove rather than a label the driver attached: each part names its
    /// author (`rlm`), its digest, and — for the two parts that land in
    /// tables — what landed.
    pub fn journal_entry(&self, rules_version: u32) -> Value {
        serde_json::json!({
            "source": "rlm",
            "digest": self.digest(),
            "parts": {
                "rules": {
                    "source": "rlm",
                    "version": rules_version,
                    "digest": self.part_digest("rules"),
                },
                "migrations": {
                    "source": "rlm",
                    "digest": self.part_digest("migrations"),
                    "names": self.migrations.iter().map(|m| m.name.clone()).collect::<Vec<_>>(),
                },
                "apis": {
                    "source": "rlm",
                    "digest": self.part_digest("apis"),
                    "routes": self.apis.iter()
                        .map(|a| format!("{} /{}", a.method.trim().to_ascii_uppercase(), a.path))
                        .collect::<Vec<_>>(),
                },
                "submission_format": {
                    "source": "rlm",
                    "digest": self.part_digest("submission_format"),
                },
                "pin_policy": {
                    "source": "rlm",
                    "digest": self.part_digest("pin_policy"),
                },
            },
        })
    }
}

/// SHA-256 of a value's canonical JSON, `sha256:<hex>`.
pub fn digest_of(value: &Value) -> String {
    let canonical = proof_canon::canonical_json(value);
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

/// Domain-separated digest over a serialisable value.
fn domain_digest<T: Serialize>(domain: &[u8], value: &T) -> String {
    let value = serde_json::to_value(value).unwrap_or(Value::Null);
    let mut h = Sha256::new();
    h.update(domain);
    h.update([0xff]);
    h.update(proof_canon::canonical_json(&value).as_bytes());
    hex::encode(h.finalize())
}

/// Read an RLM's `authoring.json`.
pub fn from_json(body: &str) -> Result<TopicAuthoring, AuthoringError> {
    serde_json::from_str(body).map_err(|e| AuthoringError::Encode(format!("parse: {e}")))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use proof_task::ChecklistRule;

    fn topic() -> TopicDocument {
        let mut doc = TopicDocument {
            id: "fixture-topic".into(),
            statement: "Score the pinned pack with the pinned runner.".into(),
            ..TopicDocument::default()
        };
        doc.metric.family = proof_task::MetricFamily::Custom;
        doc.metric.custom_id = "fixture_metric".into();
        doc
    }

    fn complete(topic_id: &str) -> TopicAuthoring {
        TopicAuthoring {
            schema_version: AUTHORING_SCHEMA,
            topic_id: topic_id.into(),
            rules: vec![ChecklistRule {
                id: "no_short_circuit".into(),
                text: "the harness must run the task".into(),
            }],
            migrations: vec![AuthoredMigration {
                name: "0001_scratch".into(),
                sql: format!(
                    "CREATE TABLE {}_scratch (id TEXT)",
                    topic_id.replace('-', "_")
                ),
            }],
            apis: vec![AuthoredApi {
                path: "status".into(),
                method: "get".into(),
                summary: "topic status".into(),
            }],
            submission_format: serde_json::json!({"kind": "tar", "max_bytes": 5_242_880}),
            pin_policy: PinPolicy {
                epsilon_nll_min: Some(0.02),
                ..PinPolicy::none()
            },
        }
    }

    #[test]
    fn a_complete_set_validates_and_is_recognised_as_complete() {
        let set = complete("fixture-topic");
        assert!(set.is_complete());
        assert!(set.missing_parts().is_empty());
        set.validate("fixture-topic").expect("validates");
        assert!(set.digest().len() == 64);
        assert!(set.part_digest("rules").starts_with("sha256:"));
        let json = set.as_section().expect("section");
        assert!(json.contains("\"pin_policy\""));
        assert!(
            !json.contains("handler"),
            "a handler is the operator's: {json}"
        );
        let back = from_json(&serde_json::to_string(&set).expect("json")).expect("round trip");
        assert_eq!(back, set);
        assert_eq!(back.digest(), set.digest());
    }

    /// Every part is load-bearing: dropping one is a refusal that names it,
    /// never a set the install fills in from somewhere else.
    #[test]
    fn a_missing_part_is_refused_by_name() {
        for (part, mutate) in [
            (
                "rules",
                Box::new(|s: &mut TopicAuthoring| s.rules.clear())
                    as Box<dyn Fn(&mut TopicAuthoring)>,
            ),
            (
                "migrations",
                Box::new(|s: &mut TopicAuthoring| s.migrations.clear()),
            ),
            ("apis", Box::new(|s: &mut TopicAuthoring| s.apis.clear())),
            (
                "submission_format",
                Box::new(|s: &mut TopicAuthoring| s.submission_format = serde_json::json!({})),
            ),
        ] {
            let mut set = complete("fixture-topic");
            mutate(&mut set);
            assert!(!set.is_complete(), "{part}");
            assert_eq!(set.missing_parts(), [part], "{part}");
            let err = set.validate("fixture-topic").expect_err(part);
            assert!(
                matches!(err, AuthoringError::MissingPart { part: got } if got == part),
                "{part}: {err}"
            );
            assert!(err.to_string().contains(part), "{err}");
        }
    }

    /// `pin_policy` is present or the set does not parse: a policy that
    /// tightens nothing is a legitimate answer, but *omitting* the part is
    /// not — that is an RLM that never considered it.
    #[test]
    fn the_pin_policy_part_is_required_even_when_it_tightens_nothing() {
        let set = complete("fixture-topic");
        let json = serde_json::to_string(&set).expect("json");
        assert!(json.contains("\"pin_policy\""), "{json}");
        let value: serde_json::Value = serde_json::from_str(&json).expect("value");
        let mut without = value.clone();
        without
            .as_object_mut()
            .expect("object")
            .remove("pin_policy");
        let err = from_json(&without.to_string()).expect_err("the part is required");
        assert!(
            err.to_string().contains("pin_policy"),
            "the refusal names the part: {err}"
        );
        // An empty policy is accepted: it says "this topic tightens nothing".
        let mut empty = value;
        empty["pin_policy"] = serde_json::json!({});
        let parsed = from_json(&empty.to_string()).expect("an empty policy parses");
        assert!(parsed.pin_policy.is_empty());
        assert!(parsed.is_complete(), "an empty policy is a complete part");
        parsed.validate("fixture-topic").expect("and it validates");
    }

    #[test]
    fn a_set_for_another_topic_or_schema_is_refused() {
        let mut set = complete("fixture-topic");
        set.topic_id = "other-topic".into();
        assert!(matches!(
            set.validate("fixture-topic"),
            Err(AuthoringError::WrongTopic { .. })
        ));
        let mut set = complete("fixture-topic");
        set.schema_version = 2;
        assert!(matches!(
            set.validate("fixture-topic"),
            Err(AuthoringError::WrongSchema { got: 2 })
        ));
    }

    /// The deny-list runs where the RLM's answer arrives: a migration that
    /// would be refused at install never becomes a job output.
    #[test]
    fn an_authored_migration_is_held_to_the_deny_list() {
        let mut set = complete("fixture-topic");
        set.migrations = vec![AuthoredMigration {
            name: "0001_bad".into(),
            sql: "DROP TABLE proof_rule_version".into(),
        }];
        let err = set.validate("fixture-topic").expect_err("denied");
        assert!(
            matches!(err, AuthoringError::MigrationDenied { .. }),
            "{err}"
        );
        assert!(err.to_string().contains("proof_rule_version"), "{err}");

        // A sibling topic's namespace is not this topic's either.
        let mut set = complete("fixture-topic");
        set.migrations = vec![AuthoredMigration {
            name: "0001_other".into(),
            sql: "CREATE TABLE some_other_topic_scratch (id TEXT)".into(),
        }];
        assert!(matches!(
            set.validate("fixture-topic"),
            Err(AuthoringError::MigrationDenied { .. })
        ));
    }

    #[test]
    fn an_authored_route_cannot_escape_the_topics_prefix_or_the_admin_namespace() {
        for bad in [
            "/v1/admin/proof/topics",
            "../admin",
            "a/../../b",
            "a//b",
            "a/./b",
            "a\\b",
            "",
            "a?x=1",
        ] {
            let mut set = complete("fixture-topic");
            set.apis = vec![AuthoredApi {
                path: bad.into(),
                method: "GET".into(),
                summary: String::new(),
            }];
            assert!(
                matches!(
                    set.validate("fixture-topic"),
                    Err(AuthoringError::Api { .. })
                ),
                "{bad:?}"
            );
        }
        let mut set = complete("fixture-topic");
        set.apis = vec![AuthoredApi {
            path: "status".into(),
            method: "TRACE".into(),
            summary: String::new(),
        }];
        assert!(matches!(
            set.validate("fixture-topic"),
            Err(AuthoringError::Api { .. })
        ));
        assert!(TopicAuthoring::is_reserved_api_path("v1/admin/x"));
        assert!(!TopicAuthoring::is_reserved_api_path("v1/administrator"));
    }

    /// The tightening rule, field by field: a policy may raise a floor, lower
    /// a ceiling, and may never do the opposite.
    #[test]
    fn a_pin_policy_may_tighten_and_never_loosen() {
        let pin = proof_task::ProofPin {
            eval_image_digest: format!("sha256:{}", "ab".repeat(32)),
            ..proof_task::ProofPin::default()
        };
        // Tightening is accepted.
        let tight = PinPolicy {
            epsilon_nll_min: Some(pin.epsilon_nll_min + 0.01),
            max_proof_deadline_s: Some(pin.max_proof_deadline_s_ceiling - 1),
            flops_budget_max: Some(pin.flops_budget_max - 1),
            holdout_size: Some(pin.holdout_size),
            eval_image_digest: Some(pin.eval_image_digest.clone()),
            gpu_class: Some(pin.gpu_class.clone()),
            ..PinPolicy::none()
        };
        tight.tightens(&pin).expect("tightens");

        for (field, policy) in [
            (
                "epsilon_nll_min",
                PinPolicy {
                    epsilon_nll_min: Some(pin.epsilon_nll_min / 2.0),
                    ..PinPolicy::none()
                },
            ),
            (
                "max_proof_deadline_s",
                PinPolicy {
                    max_proof_deadline_s: Some(pin.max_proof_deadline_s_ceiling + 1),
                    ..PinPolicy::none()
                },
            ),
            (
                "flops_budget_max",
                PinPolicy {
                    flops_budget_max: Some(pin.flops_budget_max + 1),
                    ..PinPolicy::none()
                },
            ),
        ] {
            let err = policy.tightens(&pin).expect_err(field);
            assert!(err.to_string().contains(field), "{field}: {err}");
        }
        // Equalities disagreeing with the pin are refused, not ignored: a
        // topic may not score on another image or another machine class.
        for (field, policy) in [
            (
                "eval_image_digest",
                PinPolicy {
                    eval_image_digest: Some(format!("sha256:{}", "cd".repeat(32))),
                    ..PinPolicy::none()
                },
            ),
            (
                "gpu_class",
                PinPolicy {
                    gpu_class: Some("8x".into()),
                    ..PinPolicy::none()
                },
            ),
            (
                "holdout_size",
                PinPolicy {
                    holdout_size: Some(pin.holdout_size + 1),
                    ..PinPolicy::none()
                },
            ),
        ] {
            let err = policy.tightens(&pin).expect_err(field);
            assert!(err.to_string().contains(field), "{field}: {err}");
        }
        // Shape: a zero deadline, a zero budget, a non-finite floor.
        for policy in [
            PinPolicy {
                max_proof_deadline_s: Some(0),
                ..PinPolicy::none()
            },
            PinPolicy {
                flops_budget_max: Some(0),
                ..PinPolicy::none()
            },
            PinPolicy {
                epsilon_nll_min: Some(f64::NAN),
                ..PinPolicy::none()
            },
            PinPolicy {
                epsilon_nll_min: Some(0.0),
                ..PinPolicy::none()
            },
        ] {
            assert!(policy.validate_shape().is_err(), "{policy:?}");
        }
        assert!(PinPolicy::none().is_empty());
        PinPolicy::none()
            .tightens(&pin)
            .expect("nothing to tighten");
    }

    /// The guest has the topic but not the pin, so it holds the policy to the
    /// document's own knobs.
    #[test]
    fn a_pin_policy_is_also_held_to_the_signed_document() {
        let doc = topic();
        let wider = PinPolicy {
            flops_budget_max: Some(doc.flops_budget + 1),
            ..PinPolicy::none()
        };
        let err = wider
            .tightens_document(&doc)
            .expect_err("wider than the document");
        assert!(err.to_string().contains("flops_budget_max"), "{err}");
        let ok = PinPolicy {
            flops_budget_max: Some(doc.flops_budget - 1),
            ..PinPolicy::none()
        };
        ok.tightens_document(&doc).expect("tighter");
        let holdout = PinPolicy {
            holdout_size: Some(doc.holdout_size + 1),
            ..PinPolicy::none()
        };
        assert!(holdout.tightens_document(&doc).is_err());
    }

    /// The journal entry is the proof: every part names its author and its
    /// digest, so "the RLM authored this" is a fact rather than a label.
    #[test]
    fn the_journal_entry_names_every_part_and_its_author() {
        let set = complete("fixture-topic");
        let entry = set.journal_entry(7);
        assert_eq!(entry["source"], "rlm");
        assert_eq!(entry["digest"], set.digest());
        for part in PARTS {
            assert_eq!(
                entry["parts"][part]["source"], "rlm",
                "{part} must name its author"
            );
            assert!(
                entry["parts"][part]["digest"]
                    .as_str()
                    .is_some_and(|d| d.starts_with("sha256:")),
                "{part} must carry a digest"
            );
        }
        assert_eq!(entry["parts"]["rules"]["version"], 7);
        assert_eq!(entry["parts"]["migrations"]["names"][0], "0001_scratch");
        assert_eq!(entry["parts"]["apis"]["routes"][0], "GET /status");
    }

    /// Digests are over canonical JSON, so key order does not move them, and
    /// a changed part does.
    #[test]
    fn part_digests_are_canonical_and_track_content() {
        let a = complete("fixture-topic");
        let mut b = a.clone();
        b.submission_format = serde_json::json!({"max_bytes": 5_242_880, "kind": "tar"});
        assert_eq!(
            a.part_digest("submission_format"),
            b.part_digest("submission_format"),
            "canonical JSON is order-independent"
        );
        b.submission_format = serde_json::json!({"kind": "tar", "max_bytes": 6});
        assert_ne!(
            a.part_digest("submission_format"),
            b.part_digest("submission_format")
        );
        assert_ne!(a.digest(), b.digest());
    }

    #[test]
    fn a_part_of_the_wrong_kind_is_refused_not_coerced() {
        let mut set = complete("fixture-topic");
        set.submission_format = serde_json::json!([1, 2, 3]);
        assert!(matches!(
            set.validate("fixture-topic"),
            Err(AuthoringError::SubmissionFormat { .. })
        ));
        let many = TopicAuthoring {
            migrations: (0..=MAX_MIGRATIONS)
                .map(|i| AuthoredMigration {
                    name: format!("m{i}"),
                    sql: "SELECT 1".into(),
                })
                .collect(),
            ..complete("fixture-topic")
        };
        assert!(matches!(
            many.validate("fixture-topic"),
            Err(AuthoringError::TooMany {
                part: "migrations",
                ..
            })
        ));
        assert_eq!(PARTS.len(), 5);
    }
}
