//! Operator-published topic documents: the Proof unit of work.
//!
//! A topic is a research problem, signed by the `proof` trust-root key:
//! statement, machine-checkable constraints, a metric family, a FLOP (and for
//! throughput, wall-clock) budget, a **sealed** baseline, and a holdout
//! commitment. Miners submit against `topic_id`. Publishing is an operator
//! ceremony; nothing miner-writable reaches this type.
//!
//! Three rules carry the incentive:
//!
//! - **A topic may tighten a floor, never loosen it.** Every epsilon is
//!   checked against [`crate::ProofPin`], so a topic cannot buy itself an
//!   easier win than the trust root was signed for.
//! - **A baseline must be sealed to open.** `script_sha256` names the exact
//!   baseline script bytes inside the pinned image and `metrics_commitment`
//!   names the measured metric vector. Missing either means the topic cannot
//!   reach `open`, so nobody is paid for beating a number nobody measured.
//! - **An AdamW baseline is the pinned AdamW.** A topic that ships a weaker
//!   AdamW than the locked recipe is a strawman, and a strawman is a publish
//!   reject rather than a cheap win.
//!
//! Field layout note: `epsilon_nll` and `epsilon_topic_max_regress` are
//! document-level (they exist for every family — the second is the per-split
//! regression gate), while family-specific knobs — relative epsilon, the
//! throughput quality floor, the wall budget, a custom metric id — live inside
//! [`MetricSpec`]. One knob, one home.

use serde::{Deserialize, Serialize};

use crate::{canonical_json, ProofPin, TOPIC_DOMAIN};

/// Only accepted `schema_version`.
pub const TOPIC_SCHEMA_VERSION: u32 = 1;

/// Shortest legal topic id.
pub const MIN_TOPIC_ID_LEN: usize = 2;

/// Longest legal topic id (`[a-z0-9][a-z0-9-]{1,62}`).
pub const MAX_TOPIC_ID_LEN: usize = 63;

/// Longest legal problem statement.
pub const MAX_STATEMENT_LEN: usize = 8_192;

/// The only v0 `nll`-family primary.
pub const PRIMARY_HOLDOUT_NLL: &str = "holdout_nll";

/// Throughput primary: higher is better.
pub const METRIC_TOKENS_PER_SEC: &str = "tokens_per_sec";

/// Throughput primary: lower is better.
pub const METRIC_STEP_LATENCY_MS: &str = "step_latency_ms";

/// Publication lifecycle. Only [`TopicStatus::Open`] pays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TopicStatus {
    /// Published for review; never scored, never paid.
    Draft,
    /// Live: accepts submissions and earns a share of the challenge emission.
    Open,
    /// Frozen: no new submissions, in-flight evals finish, no emission share.
    Closed,
}

/// Which measurement contract a topic is scored under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricFamily {
    /// Default LM family: beat the sealed baseline's holdout NLL.
    Nll,
    /// Systems family: beat the sealed reference's throughput or latency
    /// under the same budgets, without trading away quality.
    Throughput,
    /// A metric implemented by name inside `proof-eval`.
    Custom,
}

impl MetricFamily {
    /// Wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Nll => "nll",
            Self::Throughput => "throughput",
            Self::Custom => "custom",
        }
    }
}

/// Direction of improvement for the primary metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricDirection {
    /// Lower is better.
    Min,
    /// Higher is better.
    Max,
}

/// The metric family and its family-specific knobs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct MetricSpec {
    /// `nll` | `throughput` | `custom`.
    pub family: MetricFamily,
    /// Primary metric name the harness fills.
    pub primary: String,
    /// Improvement direction.
    pub direction: MetricDirection,
    /// Human unit label (documentation only).
    pub unit: String,
    /// Relative win required over the sealed reference (throughput / custom).
    pub epsilon_rel: f64,
    /// Largest holdout NLL a throughput topic may trade for speed
    /// (`holdout_nll <= sealed_nll + quality_floor_nll`). Required, non-zero,
    /// on the throughput family: speed is not free.
    pub quality_floor_nll: f64,
    /// Wall-clock budget in seconds. Required on the throughput family, and
    /// identical for the miner and the sealed reference.
    pub wall_budget_s: u64,
    /// Custom metric id, which must be implemented in `proof-eval`.
    pub custom_id: String,
}

impl Default for MetricSpec {
    fn default() -> Self {
        Self {
            family: MetricFamily::Nll,
            primary: PRIMARY_HOLDOUT_NLL.into(),
            direction: MetricDirection::Min,
            unit: "nats_per_token".into(),
            epsilon_rel: 0.0,
            quality_floor_nll: 0.0,
            wall_budget_s: 0,
            custom_id: String::new(),
        }
    }
}

/// The sealed baseline a challenger has to beat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Baseline {
    /// Optimizer / reference name (`adamw`, or a sealed comms reference).
    pub optimizer: String,
    /// Peak learning rate.
    pub lr: f64,
    /// Adam betas.
    pub betas: Vec<f64>,
    /// Adam epsilon.
    pub eps: f64,
    /// Decoupled weight decay.
    pub weight_decay: f64,
    /// Warmup fraction of the schedule.
    pub warmup_ratio: f64,
    /// Schedule name.
    pub schedule: String,
    /// Training dtype.
    pub dtype: String,
    /// Seed shared by the baseline and every challenger.
    pub seed: u64,
    /// FLOP budget the baseline was measured at. Must equal the topic's.
    pub flops_budget: u64,
    /// Wall budget the baseline was measured at (throughput family).
    pub wall_budget_s: u64,
    /// SHA-256 of the exact baseline script bytes inside the eval image.
    pub script_sha256: String,
    /// Commitment over the operator metric vector (`PROOF_BASELINE_FILE`).
    pub metrics_commitment: String,
    /// Free-form operator note (e.g. which fabric the reference used).
    pub notes: String,
}

impl Default for Baseline {
    fn default() -> Self {
        default_adamw(crate::FLOPS_BUDGET_MAX)
    }
}

/// The locked AdamW recipe. A topic's `adamw` baseline must be exactly this.
#[must_use]
pub fn default_adamw(flops_budget: u64) -> Baseline {
    Baseline {
        optimizer: "adamw".into(),
        lr: 3e-4,
        betas: vec![0.9, 0.95],
        eps: 1e-8,
        weight_decay: 0.1,
        warmup_ratio: 0.02,
        schedule: "cosine".into(),
        dtype: "bf16".into(),
        seed: 42,
        flops_budget,
        wall_budget_s: 0,
        script_sha256: String::new(),
        metrics_commitment: String::new(),
        notes: String::new(),
    }
}

impl Baseline {
    /// Whether both seal hashes are present and well formed.
    #[must_use]
    pub fn is_sealed(&self) -> bool {
        is_hex64(&self.script_sha256) && is_hex64(&self.metrics_commitment)
    }
}

/// Machine-checkable constraints the eval image enforces.
///
/// `deny_unknown_fields` is the point: a constraint this control plane does
/// not understand is a constraint the image cannot be trusted to enforce, so
/// an unknown key rejects the topic at publish instead of being ignored.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Constraints {
    /// No `InfiniBand` fabric.
    pub no_infiniband: bool,
    /// No NVLink between ranks.
    pub no_nvlink: bool,
    /// No NCCL all-reduce over a fast fabric.
    pub no_nccl_fast_fabric: bool,
    /// Inter-node (or emulated inter-rank) bandwidth cap in Gbit/s.
    pub max_inter_node_gbps: Option<f64>,
}

/// One signed research problem.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TopicDocument {
    /// Must equal [`TOPIC_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Immutable id (`[a-z0-9][a-z0-9-]{1,62}`). A new problem is a new id.
    pub id: String,
    /// Human problem text miners train against. Never holdout items.
    pub statement: String,
    /// Constraints the eval image enforces (it never trusts the claim).
    pub constraints: Constraints,
    /// Metric family and its knobs.
    pub metric: MetricSpec,
    /// FLOP budget for both the baseline and every challenger.
    pub flops_budget: u64,
    /// Absolute NLL a challenger must win by (`nll` family primary gate).
    pub epsilon_nll: f64,
    /// Largest per-split NLL regression tolerated on any scored stratum.
    pub epsilon_topic_max_regress: f64,
    /// Proxy override. `None` means the pin default.
    pub proxy_model: Option<String>,
    /// Sealed baseline recipe plus its two seal hashes.
    pub baseline: Baseline,
    /// Commitment over this topic's holdout records.
    pub holdout_commitment: String,
    /// Holdout record count (pin-locked at 120).
    pub holdout_size: usize,
    /// Lifecycle.
    pub status: TopicStatus,
    /// First chain epoch this topic is live.
    pub valid_from_epoch: u64,
    /// Last chain epoch, or `None` for open-ended.
    pub valid_until_epoch: Option<u64>,
    /// sr25519 signature (hex) over the canonical JSON of every other field.
    pub signature: String,
}

impl Default for TopicDocument {
    fn default() -> Self {
        Self {
            schema_version: TOPIC_SCHEMA_VERSION,
            id: String::new(),
            statement: String::new(),
            constraints: Constraints::default(),
            metric: MetricSpec::default(),
            flops_budget: crate::FLOPS_BUDGET_MAX,
            epsilon_nll: crate::EPSILON_NLL_MIN,
            epsilon_topic_max_regress: crate::EPSILON_TOPIC_MAX_REGRESS_MIN,
            proxy_model: None,
            baseline: default_adamw(crate::FLOPS_BUDGET_MAX),
            holdout_commitment: String::new(),
            holdout_size: crate::HOLDOUT_SIZE,
            status: TopicStatus::Draft,
            valid_from_epoch: 0,
            valid_until_epoch: None,
            signature: String::new(),
        }
    }
}

/// Why a topic document was refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TopicError {
    /// JSON did not parse, or carried a key this build does not understand.
    #[error("parse topic document: {0}")]
    Parse(String),
    /// Schema version drift.
    #[error("schema_version {got}, this build reads {want}")]
    WrongSchema {
        /// What the document said.
        got: u32,
        /// What this build reads.
        want: u32,
    },
    /// Id is not `[a-z0-9][a-z0-9-]{1,62}`.
    #[error("topic id {0:?} must match [a-z0-9][a-z0-9-]{{1,62}}")]
    BadId(String),
    /// Statement is empty or oversized.
    #[error("statement must be 1..={MAX_STATEMENT_LEN} chars")]
    BadStatement,
    /// A metric name outside the family's allowlist.
    #[error("metric {name:?} is not valid for family {family:?}")]
    BadMetric {
        /// Metric name the document used.
        name: String,
        /// Family it declared.
        family: &'static str,
    },
    /// A custom metric this build cannot compute.
    #[error("custom metric {0:?} is not implemented in proof-eval")]
    UnknownCustomMetric(String),
    /// A family knob is missing.
    #[error("family {family:?} requires {field}")]
    MissingFamilyField {
        /// Family that requires it.
        family: &'static str,
        /// Missing knob.
        field: &'static str,
    },
    /// A family knob was set on a family that has no such knob.
    #[error("family {family:?} must not set {field}")]
    UnexpectedFamilyField {
        /// Family that forbids it.
        family: &'static str,
        /// Offending knob.
        field: &'static str,
    },
    /// An epsilon below a pin floor.
    #[error("{field} = {got} loosens the pin floor {floor}")]
    LoosenedFloor {
        /// Which knob.
        field: &'static str,
        /// Topic value.
        got: f64,
        /// Pin floor.
        floor: f64,
    },
    /// A budget over the pin ceiling, or zero.
    #[error("flops_budget {got} must be 1..={max}")]
    BadFlopsBudget {
        /// Topic value.
        got: u64,
        /// Pin ceiling.
        max: u64,
    },
    /// Baseline and topic budgets disagree, so the comparison is not paired.
    #[error("baseline {field} {baseline} does not equal the topic's {topic}")]
    BudgetMismatch {
        /// Which budget.
        field: &'static str,
        /// Baseline value.
        baseline: u64,
        /// Topic value.
        topic: u64,
    },
    /// The baseline recipe is not the locked AdamW.
    #[error("baseline claims adamw but {field} is not the locked recipe value")]
    StrawmanBaseline {
        /// Which recipe field drifted.
        field: &'static str,
    },
    /// The baseline recipe is structurally unusable.
    #[error("baseline {field} is missing or invalid")]
    BadBaseline {
        /// Which field.
        field: &'static str,
    },
    /// The baseline is not sealed, so nothing can be scored against it.
    #[error("baseline is not sealed (script_sha256 + metrics_commitment required to open)")]
    BaselineUnsealed,
    /// Holdout commitment is not a 64-hex digest.
    #[error("holdout_commitment must be 64 hex chars")]
    BadHoldoutCommitment,
    /// Holdout size disagrees with the pin.
    #[error("holdout_size {got}, pin requires {want}")]
    BadHoldoutSize {
        /// Topic value.
        got: usize,
        /// Pin value.
        want: usize,
    },
    /// A proxy the pinned eval image does not contain.
    #[error("proxy_model {0:?} is not baked into the pinned eval image")]
    ProxyNotBaked(String),
    /// The validity window is inverted.
    #[error("valid_until_epoch {until} is before valid_from_epoch {from}")]
    BadWindow {
        /// Window start.
        from: u64,
        /// Window end.
        until: u64,
    },
    /// Signature bytes are not 64-byte hex.
    #[error("signature must be 128 hex chars")]
    BadSignatureEncoding,
    /// Signature does not verify under the pin's topic key.
    #[error("topic signature does not verify under the proof trust-root key")]
    SignatureInvalid,
    /// The pin has no usable topic key, so no topic can be trusted.
    #[error("pin carries no topic_pubkey; no topic can be verified")]
    NoTopicKey,
    /// Canonical JSON could not be built.
    #[error("canonicalize topic: {0}")]
    Canonicalize(String),
}

fn is_hex64(s: &str) -> bool {
    let t = s.trim();
    t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit())
}

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-12_f64.max(b.abs() * 1e-9)
}

/// Canonical signing bytes: the document minus `signature`.
///
/// # Errors
///
/// [`TopicError::Canonicalize`] when the document cannot be represented as
/// JSON (it always can, but the failure is surfaced rather than swallowed).
pub fn topic_signing_payload(doc: &TopicDocument) -> Result<Vec<u8>, TopicError> {
    let mut value =
        serde_json::to_value(doc).map_err(|e| TopicError::Canonicalize(e.to_string()))?;
    if let Some(obj) = value.as_object_mut() {
        obj.remove("signature");
    }
    Ok(canonical_json(&value).into_bytes())
}

impl MetricSpec {
    /// Enforce the family's allowlist and knobs.
    fn validate(&self, pin: &ProofPin, supported_custom: &[&str]) -> Result<(), TopicError> {
        let family = self.family.as_str();
        let bad_metric = || TopicError::BadMetric {
            name: self.primary.clone(),
            family,
        };
        match self.family {
            MetricFamily::Nll => {
                if self.primary.trim() != PRIMARY_HOLDOUT_NLL {
                    return Err(bad_metric());
                }
                if self.direction != MetricDirection::Min {
                    return Err(bad_metric());
                }
                for (field, set) in [
                    ("epsilon_rel", self.epsilon_rel != 0.0),
                    ("quality_floor_nll", self.quality_floor_nll != 0.0),
                    ("wall_budget_s", self.wall_budget_s != 0),
                    ("custom_id", !self.custom_id.trim().is_empty()),
                ] {
                    if set {
                        return Err(TopicError::UnexpectedFamilyField { family, field });
                    }
                }
            }
            MetricFamily::Throughput => {
                let want = match self.primary.trim() {
                    METRIC_TOKENS_PER_SEC => MetricDirection::Max,
                    METRIC_STEP_LATENCY_MS => MetricDirection::Min,
                    _ => return Err(bad_metric()),
                };
                if self.direction != want {
                    return Err(bad_metric());
                }
                if self.wall_budget_s == 0 {
                    return Err(TopicError::MissingFamilyField {
                        family,
                        field: "wall_budget_s",
                    });
                }
                // Speed is not free: a throughput topic that omits the
                // quality floor would pay for a model that got faster by
                // getting worse.
                if self.quality_floor_nll <= 0.0 {
                    return Err(TopicError::MissingFamilyField {
                        family,
                        field: "quality_floor_nll",
                    });
                }
                if self.quality_floor_nll.is_nan()
                    || self.quality_floor_nll > pin.quality_floor_nll_max
                {
                    return Err(TopicError::LoosenedFloor {
                        field: "metric.quality_floor_nll",
                        got: self.quality_floor_nll,
                        floor: pin.quality_floor_nll_max,
                    });
                }
                if self.epsilon_rel.is_nan() || self.epsilon_rel < pin.epsilon_throughput_rel_min {
                    return Err(TopicError::LoosenedFloor {
                        field: "metric.epsilon_rel",
                        got: self.epsilon_rel,
                        floor: pin.epsilon_throughput_rel_min,
                    });
                }
                if !self.custom_id.trim().is_empty() {
                    return Err(TopicError::UnexpectedFamilyField {
                        family,
                        field: "custom_id",
                    });
                }
            }
            MetricFamily::Custom => {
                let id = self.custom_id.trim();
                if id.is_empty() {
                    return Err(TopicError::MissingFamilyField {
                        family,
                        field: "custom_id",
                    });
                }
                if !supported_custom.contains(&id) {
                    return Err(TopicError::UnknownCustomMetric(id.to_owned()));
                }
                if self.primary.trim().is_empty() {
                    return Err(bad_metric());
                }
                if self.epsilon_rel.is_nan() || self.epsilon_rel <= 0.0 {
                    return Err(TopicError::MissingFamilyField {
                        family,
                        field: "epsilon_rel",
                    });
                }
            }
        }
        Ok(())
    }
}

impl Baseline {
    /// Enforce the recipe shape, the paired budgets, and the strawman rule.
    fn validate(&self, metric: &MetricSpec, topic_flops: u64) -> Result<(), TopicError> {
        let name = self.optimizer.trim().to_ascii_lowercase();
        if name.is_empty() {
            return Err(TopicError::BadBaseline { field: "optimizer" });
        }
        if self.schedule.trim().is_empty() {
            return Err(TopicError::BadBaseline { field: "schedule" });
        }
        if self.dtype.trim().is_empty() {
            return Err(TopicError::BadBaseline { field: "dtype" });
        }
        if self.betas.len() != 2 || self.betas.iter().any(|b| !b.is_finite()) {
            return Err(TopicError::BadBaseline { field: "betas" });
        }
        if !self.lr.is_finite() || self.lr <= 0.0 {
            return Err(TopicError::BadBaseline { field: "lr" });
        }
        if self.flops_budget != topic_flops {
            return Err(TopicError::BudgetMismatch {
                field: "flops_budget",
                baseline: self.flops_budget,
                topic: topic_flops,
            });
        }
        // The systems family compares wall-clock work, so an unpaired wall
        // budget would let the reference run for a different amount of time.
        if metric.family == MetricFamily::Throughput && self.wall_budget_s != metric.wall_budget_s {
            return Err(TopicError::BudgetMismatch {
                field: "wall_budget_s",
                baseline: self.wall_budget_s,
                topic: metric.wall_budget_s,
            });
        }
        if name == "adamw" {
            let locked = default_adamw(topic_flops);
            for (field, ok) in [
                ("lr", approx(self.lr, locked.lr)),
                ("betas", self.betas == locked.betas),
                ("eps", approx(self.eps, locked.eps)),
                (
                    "weight_decay",
                    approx(self.weight_decay, locked.weight_decay),
                ),
                (
                    "warmup_ratio",
                    approx(self.warmup_ratio, locked.warmup_ratio),
                ),
                ("schedule", self.schedule.trim() == locked.schedule),
                ("dtype", self.dtype.trim() == locked.dtype),
                ("seed", self.seed == locked.seed),
            ] {
                if !ok {
                    return Err(TopicError::StrawmanBaseline { field });
                }
            }
        }
        Ok(())
    }
}

impl TopicDocument {
    /// Parse an operator topic document.
    ///
    /// # Errors
    ///
    /// [`TopicError::Parse`] on malformed JSON or an unknown key — an unknown
    /// constraint is a constraint nothing enforces, so it is refused here
    /// rather than ignored.
    pub fn from_json(body: &str) -> Result<Self, TopicError> {
        serde_json::from_str(body).map_err(|e| TopicError::Parse(e.to_string()))
    }

    /// Parse a JSON array or `{ "topics": [...] }` map of documents.
    ///
    /// # Errors
    ///
    /// [`TopicError::Parse`] when the body is neither shape.
    pub fn many_from_json(body: &str) -> Result<Vec<Self>, TopicError> {
        if let Ok(list) = serde_json::from_str::<Vec<Self>>(body) {
            return Ok(list);
        }
        #[derive(Deserialize)]
        struct Wrapper {
            topics: Vec<TopicDocument>,
        }
        serde_json::from_str::<Wrapper>(body)
            .map(|w| w.topics)
            .map_err(|e| TopicError::Parse(e.to_string()))
    }

    /// Structural + floor validation. Does **not** check the signature.
    ///
    /// # Errors
    ///
    /// See [`TopicError`]. A `draft` topic with an unsealed baseline is legal;
    /// an `open` one is not.
    pub fn validate(&self, pin: &ProofPin, supported_custom: &[&str]) -> Result<(), TopicError> {
        if self.schema_version != TOPIC_SCHEMA_VERSION {
            return Err(TopicError::WrongSchema {
                got: self.schema_version,
                want: TOPIC_SCHEMA_VERSION,
            });
        }
        if !is_valid_id(&self.id) {
            return Err(TopicError::BadId(self.id.clone()));
        }
        let statement = self.statement.trim();
        if statement.is_empty() || statement.chars().count() > MAX_STATEMENT_LEN {
            return Err(TopicError::BadStatement);
        }
        self.metric.validate(pin, supported_custom)?;
        if self.flops_budget == 0 || self.flops_budget > pin.flops_budget_max {
            return Err(TopicError::BadFlopsBudget {
                got: self.flops_budget,
                max: pin.flops_budget_max,
            });
        }
        for (field, got, floor) in [
            ("epsilon_nll", self.epsilon_nll, pin.epsilon_nll_min),
            (
                "epsilon_topic_max_regress",
                self.epsilon_topic_max_regress,
                pin.epsilon_topic_max_regress_min,
            ),
        ] {
            if got.is_nan() || got < floor {
                return Err(TopicError::LoosenedFloor { field, got, floor });
            }
        }
        if let Some(proxy) = self.proxy_model.as_deref().map(str::trim) {
            if proxy.is_empty() || !pin.bakes_proxy(proxy) {
                return Err(TopicError::ProxyNotBaked(proxy.to_owned()));
            }
        }
        self.baseline.validate(&self.metric, self.flops_budget)?;
        if !is_hex64(&self.holdout_commitment) {
            return Err(TopicError::BadHoldoutCommitment);
        }
        if self.holdout_size != pin.holdout_size {
            return Err(TopicError::BadHoldoutSize {
                got: self.holdout_size,
                want: pin.holdout_size,
            });
        }
        if let Some(until) = self.valid_until_epoch {
            if until < self.valid_from_epoch {
                return Err(TopicError::BadWindow {
                    from: self.valid_from_epoch,
                    until,
                });
            }
        }
        if self.status == TopicStatus::Open && !self.baseline.is_sealed() {
            return Err(TopicError::BaselineUnsealed);
        }
        Ok(())
    }

    /// Verify the operator signature under the pin's topic key.
    ///
    /// # Errors
    ///
    /// [`TopicError::NoTopicKey`], [`TopicError::BadSignatureEncoding`], or
    /// [`TopicError::SignatureInvalid`]. A topic that does not verify is
    /// ignored: it is not a topic this subnet published.
    pub fn verify_signature(&self, pin: &ProofPin) -> Result<(), TopicError> {
        let key = pin.topic_pubkey_bytes().ok_or(TopicError::NoTopicKey)?;
        let sig =
            hex::decode(self.signature.trim()).map_err(|_| TopicError::BadSignatureEncoding)?;
        if sig.len() != crypto::SIGNATURE_LEN {
            return Err(TopicError::BadSignatureEncoding);
        }
        let payload = topic_signing_payload(self)?;
        crypto::verify_raw(&key, TOPIC_DOMAIN, &payload, &sig)
            .map_err(|_| TopicError::SignatureInvalid)
    }

    /// Sign this document with the `proof` mini-secret (ceremony helper).
    ///
    /// # Errors
    ///
    /// [`TopicError::SignatureInvalid`] when the secret is malformed.
    pub fn sign_with(&self, secret: &[u8; 32]) -> Result<String, TopicError> {
        let payload = topic_signing_payload(self)?;
        let sig = crypto::sign_raw(secret, TOPIC_DOMAIN, &payload)
            .map_err(|_| TopicError::SignatureInvalid)?;
        Ok(hex::encode(sig))
    }

    /// Whether this topic accepts submissions and earns emission at `epoch`.
    #[must_use]
    pub fn is_open_at(&self, epoch: u64) -> bool {
        self.status == TopicStatus::Open
            && epoch >= self.valid_from_epoch
            && self.valid_until_epoch.is_none_or(|u| epoch <= u)
    }

    /// Slice id bound into this topic's measurements.
    #[must_use]
    pub fn slice_id(&self) -> String {
        format!("{}-{}", crate::HOLDOUT_SLICE_PREFIX, self.id)
    }
}

fn is_valid_id(id: &str) -> bool {
    let len = id.len();
    if len < MIN_TOPIC_ID_LEN || len > MAX_TOPIC_ID_LEN {
        return false;
    }
    let mut chars = id.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{holdout_commitment, synthetic_holdout, STRATUM_SIZE};

    fn pin() -> ProofPin {
        ProofPin {
            topic_pubkey: hex::encode(crypto::public_key_from_mini_secret(&sk()).expect("pk")),
            proxy_model: "Qwen/Qwen3.8-0.6B".into(),
            proxy_models: vec!["Qwen/Qwen3.8-0.6B".into(), "Qwen/Qwen3.8-1.7B".into()],
            ..ProofPin::default()
        }
    }

    fn sk() -> [u8; 32] {
        let mut s = [3u8; 32];
        s[0] = 17;
        s
    }

    fn sealed(mut b: Baseline) -> Baseline {
        b.script_sha256 = "11".repeat(32);
        b.metrics_commitment = "22".repeat(32);
        b
    }

    fn nll_topic() -> TopicDocument {
        TopicDocument {
            id: "adamw-beater-v0".into(),
            statement: "Beat the sealed AdamW baseline's holdout NLL at the same FLOP budget."
                .into(),
            baseline: sealed(default_adamw(crate::FLOPS_BUDGET_MAX)),
            holdout_commitment: holdout_commitment(&synthetic_holdout(STRATUM_SIZE, 1)),
            status: TopicStatus::Open,
            ..TopicDocument::default()
        }
    }

    fn dt_no_ib() -> TopicDocument {
        let mut baseline = sealed(default_adamw(crate::FLOPS_BUDGET_MAX));
        baseline.wall_budget_s = 14_400;
        baseline.notes = "sealed NCCL/IB reference at the same cap".into();
        TopicDocument {
            id: "dt-no-ib-v0".into(),
            statement: "Train the pinned proxy with no InfiniBand, no NVLink, and no NCCL \
                        all-reduce over a fast fabric; inter-rank bandwidth is capped at \
                        12.5 Gbit/s."
                .into(),
            constraints: Constraints {
                no_infiniband: true,
                no_nvlink: true,
                no_nccl_fast_fabric: true,
                max_inter_node_gbps: Some(12.5),
            },
            metric: MetricSpec {
                family: MetricFamily::Throughput,
                primary: METRIC_TOKENS_PER_SEC.into(),
                direction: MetricDirection::Max,
                unit: "tokens_per_second".into(),
                epsilon_rel: 0.05,
                quality_floor_nll: 0.02,
                wall_budget_s: 14_400,
                custom_id: String::new(),
            },
            baseline,
            holdout_commitment: holdout_commitment(&synthetic_holdout(STRATUM_SIZE, 1)),
            status: TopicStatus::Open,
            ..TopicDocument::default()
        }
    }

    #[test]
    fn the_locked_nll_and_throughput_topics_validate() {
        let p = pin();
        nll_topic().validate(&p, &[]).expect("nll topic");
        dt_no_ib().validate(&p, &[]).expect("throughput topic");
    }

    #[test]
    fn signature_round_trips_and_any_edit_breaks_it() {
        let p = pin();
        let mut doc = dt_no_ib();
        doc.signature = doc.sign_with(&sk()).expect("sign");
        doc.verify_signature(&p).expect("verifies");

        let mut tampered = doc.clone();
        tampered.flops_budget -= 1;
        assert!(matches!(
            tampered.verify_signature(&p),
            Err(TopicError::SignatureInvalid)
        ));

        // Loosening an epsilon after signing is the attack this defends.
        let mut loosened = doc.clone();
        loosened.epsilon_nll = 0.0;
        assert!(matches!(
            loosened.verify_signature(&p),
            Err(TopicError::SignatureInvalid)
        ));

        let mut unsigned = doc;
        unsigned.signature = String::new();
        assert!(matches!(
            unsigned.verify_signature(&p),
            Err(TopicError::BadSignatureEncoding)
        ));
    }

    #[test]
    fn a_topic_signed_by_another_key_is_not_this_subnets_topic() {
        let p = pin();
        let mut doc = nll_topic();
        let mut other = [9u8; 32];
        other[1] = 4;
        doc.signature = doc.sign_with(&other).expect("sign");
        assert!(matches!(
            doc.verify_signature(&p),
            Err(TopicError::SignatureInvalid)
        ));
    }

    /// The signature covers everything but `signature`, so the payload must
    /// not contain the field at all.
    #[test]
    fn signing_payload_excludes_only_the_signature() {
        let mut doc = nll_topic();
        doc.signature = "ab".repeat(64);
        let payload =
            String::from_utf8(topic_signing_payload(&doc).expect("payload")).expect("utf8");
        assert!(!payload.contains("\"signature\""), "{payload}");
        for field in [
            "\"id\"",
            "\"statement\"",
            "\"metric\"",
            "\"baseline\"",
            "\"holdout_commitment\"",
            "\"status\"",
        ] {
            assert!(payload.contains(field), "missing {field} in {payload}");
        }
    }

    #[test]
    fn a_topic_may_tighten_a_floor_but_never_loosen_one() {
        let p = pin();
        let mut tight = nll_topic();
        tight.epsilon_nll = 0.05;
        tight.epsilon_topic_max_regress = 0.06;
        tight.validate(&p, &[]).expect("tighter is fine");

        for field in ["epsilon_nll", "epsilon_topic_max_regress"] {
            let mut loose = nll_topic();
            if field == "epsilon_nll" {
                loose.epsilon_nll = 0.01;
            } else {
                loose.epsilon_topic_max_regress = 0.04;
            }
            assert!(
                matches!(
                    loose.validate(&p, &[]),
                    Err(TopicError::LoosenedFloor { .. })
                ),
                "{field} must not be loosened"
            );
        }

        let mut big = nll_topic();
        big.flops_budget = crate::FLOPS_BUDGET_MAX + 1;
        assert!(matches!(
            big.validate(&p, &[]),
            Err(TopicError::BadFlopsBudget { .. })
        ));
    }

    /// The headline publish gate: a topic that ships a hobbled AdamW would
    /// let any miner "prove" an improvement.
    #[test]
    fn a_strawman_adamw_baseline_is_a_publish_reject() {
        let p = pin();
        for mutate in 0..4 {
            let mut doc = nll_topic();
            match mutate {
                0 => doc.baseline.lr = 3e-6,
                1 => doc.baseline.warmup_ratio = 0.0,
                2 => doc.baseline.schedule = "constant".into(),
                _ => doc.baseline.betas = vec![0.9, 0.999],
            }
            assert!(
                matches!(
                    doc.validate(&p, &[]),
                    Err(TopicError::StrawmanBaseline { .. })
                ),
                "case {mutate} must be refused"
            );
        }
    }

    /// A topic may seal a non-AdamW reference (the comms baseline), but the
    /// budgets still have to be paired.
    #[test]
    fn a_sealed_non_adamw_reference_is_allowed_with_paired_budgets() {
        let p = pin();
        let mut doc = dt_no_ib();
        doc.baseline.optimizer = "nccl-ib-reference".into();
        doc.baseline.lr = 1e-3;
        doc.validate(&p, &[]).expect("sealed comms reference");

        doc.baseline.wall_budget_s = 60;
        assert!(matches!(
            doc.validate(&p, &[]),
            Err(TopicError::BudgetMismatch {
                field: "wall_budget_s",
                ..
            })
        ));
    }

    #[test]
    fn an_unsealed_baseline_can_draft_but_never_open() {
        let p = pin();
        let mut doc = nll_topic();
        doc.baseline.metrics_commitment = String::new();
        assert!(matches!(
            doc.validate(&p, &[]),
            Err(TopicError::BaselineUnsealed)
        ));
        doc.status = TopicStatus::Draft;
        doc.validate(&p, &[]).expect("a draft may be unsealed");
        assert!(!doc.is_open_at(0));
    }

    #[test]
    fn throughput_requires_a_wall_budget_and_a_quality_floor() {
        let p = pin();
        let mut no_wall = dt_no_ib();
        no_wall.metric.wall_budget_s = 0;
        no_wall.baseline.wall_budget_s = 0;
        assert!(matches!(
            no_wall.validate(&p, &[]),
            Err(TopicError::MissingFamilyField {
                field: "wall_budget_s",
                ..
            })
        ));

        let mut free_speed = dt_no_ib();
        free_speed.metric.quality_floor_nll = 0.0;
        assert!(matches!(
            free_speed.validate(&p, &[]),
            Err(TopicError::MissingFamilyField {
                field: "quality_floor_nll",
                ..
            })
        ));

        let mut cheap_quality = dt_no_ib();
        cheap_quality.metric.quality_floor_nll = 1.0;
        assert!(matches!(
            cheap_quality.validate(&p, &[]),
            Err(TopicError::LoosenedFloor { .. })
        ));

        let mut weak = dt_no_ib();
        weak.metric.epsilon_rel = 0.01;
        assert!(matches!(
            weak.validate(&p, &[]),
            Err(TopicError::LoosenedFloor { .. })
        ));
    }

    #[test]
    fn metric_names_are_family_allowlisted() {
        let p = pin();
        let mut wrong_primary = nll_topic();
        wrong_primary.metric.primary = "tokens_per_sec".into();
        assert!(matches!(
            wrong_primary.validate(&p, &[]),
            Err(TopicError::BadMetric { .. })
        ));

        let mut wrong_direction = dt_no_ib();
        wrong_direction.metric.direction = MetricDirection::Min;
        assert!(matches!(
            wrong_direction.validate(&p, &[]),
            Err(TopicError::BadMetric { .. })
        ));

        let mut latency = dt_no_ib();
        latency.metric.primary = METRIC_STEP_LATENCY_MS.into();
        latency.metric.direction = MetricDirection::Min;
        latency
            .validate(&p, &[])
            .expect("latency is a legal primary");

        let mut knob_on_nll = nll_topic();
        knob_on_nll.metric.epsilon_rel = 0.2;
        assert!(matches!(
            knob_on_nll.validate(&p, &[]),
            Err(TopicError::UnexpectedFamilyField { .. })
        ));
    }

    /// A custom metric nothing implements is a 400 at publish, never a
    /// silently-skipped gate.
    #[test]
    fn custom_metrics_must_be_implemented() {
        let p = pin();
        let mut doc = nll_topic();
        doc.metric = MetricSpec {
            family: MetricFamily::Custom,
            primary: "bits_per_joule".into(),
            direction: MetricDirection::Min,
            unit: "bits/J".into(),
            epsilon_rel: 0.10,
            quality_floor_nll: 0.0,
            wall_budget_s: 0,
            custom_id: "bits_per_joule".into(),
        };
        assert!(matches!(
            doc.validate(&p, &[]),
            Err(TopicError::UnknownCustomMetric(_))
        ));
        doc.validate(&p, &["bits_per_joule"])
            .expect("implemented custom metric");
    }

    #[test]
    fn ids_are_immutable_slugs() {
        let p = pin();
        for bad in ["", "a", "-lead", "Upper", "has_underscore", &"x".repeat(64)] {
            let mut doc = nll_topic();
            doc.id = bad.to_owned();
            assert!(
                matches!(doc.validate(&p, &[]), Err(TopicError::BadId(_))),
                "{bad:?} must be refused"
            );
        }
        for good in ["dt-no-ib-v0", "ab", "0x-topic"] {
            let mut doc = nll_topic();
            doc.id = good.to_owned();
            doc.validate(&p, &[])
                .unwrap_or_else(|e| panic!("{good}: {e}"));
        }
    }

    #[test]
    fn an_unknown_constraint_key_is_refused_not_ignored() {
        let body = r#"{
            "schema_version": 1, "id": "dt-no-ib-v0", "statement": "x",
            "constraints": { "no_infiniband": true, "allow_secret_fabric": true },
            "metric": {}, "flops_budget": 1, "epsilon_nll": 0.02,
            "epsilon_topic_max_regress": 0.05, "proxy_model": null,
            "baseline": {}, "holdout_commitment": "", "holdout_size": 120,
            "status": "draft", "valid_from_epoch": 0, "valid_until_epoch": null,
            "signature": ""
        }"#;
        let err = TopicDocument::from_json(body).expect_err("unknown constraint");
        assert!(
            format!("{err}").contains("allow_secret_fabric"),
            "the refusal must name the key: {err}"
        );
    }

    #[test]
    fn a_proxy_the_image_does_not_bake_is_refused() {
        let p = pin();
        let mut doc = nll_topic();
        doc.proxy_model = Some("Qwen/Qwen3.8-1.7B".into());
        doc.validate(&p, &[]).expect("a baked proxy is fine");
        doc.proxy_model = Some("Qwen/Qwen3.8-27B".into());
        assert!(matches!(
            doc.validate(&p, &[]),
            Err(TopicError::ProxyNotBaked(_))
        ));
    }

    #[test]
    fn the_validity_window_gates_open() {
        let mut doc = nll_topic();
        doc.valid_from_epoch = 10;
        doc.valid_until_epoch = Some(20);
        assert!(!doc.is_open_at(9));
        assert!(doc.is_open_at(10));
        assert!(doc.is_open_at(20));
        assert!(!doc.is_open_at(21));

        doc.valid_until_epoch = None;
        assert!(doc.is_open_at(u64::MAX));

        doc.valid_until_epoch = Some(5);
        assert!(matches!(
            doc.validate(&pin(), &[]),
            Err(TopicError::BadWindow { .. })
        ));
    }

    #[test]
    fn documents_parse_from_an_array_or_a_wrapper() {
        let one = nll_topic();
        let array = serde_json::to_string(&vec![one.clone()]).expect("json");
        assert_eq!(
            TopicDocument::many_from_json(&array).expect("array").len(),
            1
        );
        let wrapped = format!("{{\"topics\":{array}}}");
        assert_eq!(
            TopicDocument::many_from_json(&wrapped)
                .expect("wrapper")
                .len(),
            1
        );
        assert!(TopicDocument::many_from_json("nope").is_err());
    }
}
