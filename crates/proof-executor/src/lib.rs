//! Proof eval **executor** offer: the Lium machine class the digest-pinned
//! `proof-eval` image is rented on.
//!
//! Sibling of the RLM judge [`proof_task::InferenceOffer`] — a separate
//! document with its own commitment, admin route, and pin ceilings. The
//! judge is *what* scores; the executor is *where* the proof runs. Git carries
//! only ceilings ([`proof_task::ProofPin`]: `gpu_class`,
//! `max_proof_deadline_s_ceiling`, `allowed_lium_template_prefixes`); the
//! live offer is operator state (`PROOF_EVAL_EXECUTOR_OFFER_FILE`, rotated
//! with `POST /v1/admin/proof/executor`). Miners never bind it.
//!
//! Every refusal here is fail-closed: a missing, closed, or non-`1x` offer
//! means `can_score=false` and submits answer **503**. Nothing here rents.
//!
//! Isolation invariants this contract assumes and never weakens: the
//! control-plane host runs neither the eval image nor the RLM judge; the
//! harvest is the **only** path from the control plane to a rented GPU; an
//! offer names a remote machine class, never a host process or a specific
//! machine. The contract is challenge-agnostic — it carries no topic ids,
//! benchmark names, or model names, only ceilings the operator publishes.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::must_use_candidate
)]

mod plan;

use proof_task::{canonical_json, is_hex64, is_slug, ProofPin, TopicDocument};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use plan::{
    executor_plan, ExecutorPlan, HarvestOverrides, HARVEST_DEADLINE_SECS_ENV,
    HARVEST_GPU_COUNT_ENV, HARVEST_TEMPLATE_ID_ENV,
};
pub use proof_task::OfferStatus;

/// Env var naming the live offer file (operator state, never git).
pub const EVAL_EXECUTOR_OFFER_FILE_ENV: &str = "PROOF_EVAL_EXECUTOR_OFFER_FILE";

/// Longest `lium_template_id` an offer may carry.
pub const MAX_TEMPLATE_ID_LEN: usize = 128;

/// Operator live offer (`PROOF_EVAL_EXECUTOR_OFFER_FILE`). Never committed.
///
/// Every field is public: there is no origin or credential in this document,
/// so `/v1/status` and `/v1/proof/executor` may show it whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalExecutorOffer {
    /// Immutable slug identifying this executor.
    pub offer_id: String,
    /// Lium template the harvest rents. Either the digest-scoped template
    /// name (`proof-eval-<12 hex of the digest>`, resolved or created bound
    /// to `repo@digest`) or a raw Lium template UUID. The pin's
    /// `allowed_lium_template_prefixes` decides which forms are legal.
    pub lium_template_id: String,
    /// Machine class. Must equal the pin `gpu_class` (`1x`).
    pub machine_shape: String,
    /// Longest proof run on this executor, seconds. `<=` the pin ceiling.
    pub max_proof_deadline_s: u64,
    /// Eval image digest this offer was validated for. Empty = unbound;
    /// non-empty must equal the pin digest.
    #[serde(default)]
    pub eval_image_digest: String,
    /// `sha256` hex of canonical JSON of the public knobs
    /// ([`executor_config_commitment`]).
    pub config_commitment: String,
    /// `open` | `closed`.
    pub status: OfferStatus,
}

/// Why an executor offer was refused or cannot score.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExecutorOfferError {
    /// JSON did not parse, or carried an unknown key.
    #[error("parse eval executor offer: {0}")]
    Parse(String),
    /// `offer_id` is not a slug.
    #[error("offer_id {0:?} must match [a-z0-9][a-z0-9-]{{1,62}}")]
    BadId(String),
    /// `lium_template_id` is empty, oversized, or not printable ASCII.
    #[error("lium_template_id must be 1..={MAX_TEMPLATE_ID_LEN} printable ASCII chars")]
    BadTemplateId,
    /// `lium_template_id` is outside the pin allowlist.
    #[error("lium_template_id {0:?} is not under the pin allowed_lium_template_prefixes")]
    TemplateNotAllowed(String),
    /// A digest-scoped template name does not name the pinned digest.
    #[error("lium_template_id {0:?} does not carry the pinned eval image digest prefix {1}")]
    TemplateDigestMismatch(String, String),
    /// Offer shape is not the pin `gpu_class`.
    #[error("machine_shape {got:?} is not the pin gpu_class {want:?}; refuse scoring")]
    ShapeMismatch {
        /// What the offer said.
        got: String,
        /// The only legal shape.
        want: String,
    },
    /// Deadline is zero or above the ceiling.
    #[error("max_proof_deadline_s = {0} must be 1..={1}")]
    BadDeadline(u64, u64),
    /// Offer names an eval image digest that is not the pin.
    #[error("eval_image_digest does not match the pin")]
    DigestMismatch,
    /// Declared commitment is not 64 hex or does not match the knobs.
    #[error("config_commitment does not match sha256(canonical executor config)")]
    CommitmentMismatch,
    /// No offer loaded on this host.
    #[error("eval executor offer missing; refuse scoring")]
    Missing,
    /// Offer is present but closed.
    #[error("eval executor offer is closed; refuse scoring")]
    Closed,
    /// Open offer cannot serve this topic (commitment pin mismatch).
    #[error("open eval executor offer cannot serve topic eval_executor constraints")]
    CannotServeTopic,
    /// An operator env override is set but unusable. Fail closed, never
    /// silently fall back to the offer.
    #[error("{0} is set but not a usable harvest override")]
    BadOverride(&'static str),
    /// The rent would not be exactly the pinned width.
    #[error("abort: executor would rent {0}x GPUs; the Proof executor is exactly {1}x")]
    GpuCount(u32, u32),
}

#[derive(Serialize)]
struct CommitmentMaterial<'a> {
    eval_image_digest: &'a str,
    lium_template_id: &'a str,
    machine_shape: &'a str,
    max_proof_deadline_s: u64,
}

/// `sha256` hex of canonical JSON of the public executor knobs.
///
/// `offer_id` and `status` are lifecycle, not configuration, and stay out.
pub fn executor_config_commitment(
    lium_template_id: &str,
    machine_shape: &str,
    max_proof_deadline_s: u64,
    eval_image_digest: &str,
) -> String {
    let material = CommitmentMaterial {
        eval_image_digest: eval_image_digest.trim(),
        lium_template_id: lium_template_id.trim(),
        machine_shape: machine_shape.trim(),
        max_proof_deadline_s,
    };
    let value = serde_json::to_value(&material).unwrap_or(serde_json::Value::Null);
    let mut h = Sha256::new();
    h.update(canonical_json(&value).as_bytes());
    hex::encode(h.finalize())
}

/// GPUs behind a machine shape such as `1x` (`None` when not `<n>x`, n >= 1).
pub fn shape_gpu_count(shape: &str) -> Option<u32> {
    let n: u32 = shape.trim().strip_suffix('x')?.parse().ok()?;
    (n >= 1).then_some(n)
}

/// `8-4-4-4-12` lowercase/uppercase hex: a raw Lium template id rather than a
/// digest-scoped template name.
pub fn is_lium_template_uuid(id: &str) -> bool {
    let t = id.trim();
    t.len() == 36
        && t.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

/// Template identifier shape, pin allowlist, and (for a digest-scoped name)
/// the pinned digest prefix.
pub fn check_template_id(pin: &ProofPin, template_id: &str) -> Result<(), ExecutorOfferError> {
    let id = template_id.trim();
    if id.is_empty() || id.len() > MAX_TEMPLATE_ID_LEN || !id.bytes().all(|b| b.is_ascii_graphic())
    {
        return Err(ExecutorOfferError::BadTemplateId);
    }
    if !pin.allows_template(id) {
        return Err(ExecutorOfferError::TemplateNotAllowed(id.to_owned()));
    }
    if !is_lium_template_uuid(id) && pin.can_rent() {
        let hex = pin.eval_image_digest.trim().trim_start_matches("sha256:");
        let prefix = hex.get(..12).unwrap_or(hex);
        if !id.contains(prefix) {
            return Err(ExecutorOfferError::TemplateDigestMismatch(
                id.to_owned(),
                prefix.to_owned(),
            ));
        }
    }
    Ok(())
}

impl EvalExecutorOffer {
    /// Parse one operator offer document.
    ///
    /// # Errors
    ///
    /// [`ExecutorOfferError::Parse`] on malformed JSON or an unknown key.
    pub fn from_json(body: &str) -> Result<Self, ExecutorOfferError> {
        serde_json::from_str(body).map_err(|e| ExecutorOfferError::Parse(e.to_string()))
    }

    /// The commitment this offer's knobs hash to.
    pub fn expected_commitment(&self) -> String {
        executor_config_commitment(
            &self.lium_template_id,
            &self.machine_shape,
            self.max_proof_deadline_s,
            &self.eval_image_digest,
        )
    }

    /// Structural check against the pin. Does not require `open`.
    ///
    /// # Errors
    ///
    /// See [`ExecutorOfferError`]. A closed-but-valid offer is legal to load.
    pub fn validate(&self, pin: &ProofPin) -> Result<(), ExecutorOfferError> {
        if !is_slug(&self.offer_id) {
            return Err(ExecutorOfferError::BadId(self.offer_id.clone()));
        }
        check_template_id(pin, &self.lium_template_id)?;
        if self.machine_shape.trim() != pin.gpu_class.trim() {
            return Err(ExecutorOfferError::ShapeMismatch {
                got: self.machine_shape.clone(),
                want: pin.gpu_class.clone(),
            });
        }
        if self.max_proof_deadline_s == 0
            || self.max_proof_deadline_s > pin.max_proof_deadline_s_ceiling
        {
            return Err(ExecutorOfferError::BadDeadline(
                self.max_proof_deadline_s,
                pin.max_proof_deadline_s_ceiling,
            ));
        }
        let digest = self.eval_image_digest.trim();
        if !digest.is_empty() && !digest.eq_ignore_ascii_case(pin.eval_image_digest.trim()) {
            return Err(ExecutorOfferError::DigestMismatch);
        }
        if !is_hex64(&self.config_commitment)
            || !self
                .config_commitment
                .eq_ignore_ascii_case(&self.expected_commitment())
        {
            return Err(ExecutorOfferError::CommitmentMismatch);
        }
        Ok(())
    }

    /// Whether this executor is open for rents.
    pub fn is_open(&self) -> bool {
        self.status == OfferStatus::Open
    }

    /// GPUs this offer rents (`1` for `1x`).
    pub fn gpu_count(&self) -> Option<u32> {
        shape_gpu_count(&self.machine_shape)
    }

    /// Public status payload. Every field is public; still no file paths.
    pub fn public_view(&self) -> serde_json::Value {
        serde_json::json!({
            "offer_id": self.offer_id,
            "lium_template_id": self.lium_template_id,
            "machine_shape": self.machine_shape,
            "gpu_count": self.gpu_count(),
            "max_proof_deadline_s": self.max_proof_deadline_s,
            "eval_image_digest": self.eval_image_digest,
            "config_commitment": self.config_commitment,
            "status": self.status,
        })
    }

    /// Whether this open executor may run `topic`.
    ///
    /// A topic only tightens the deadline (taken as a minimum at plan time)
    /// and may pin the live offer's commitment.
    ///
    /// # Errors
    ///
    /// [`ExecutorOfferError::Closed`] or [`ExecutorOfferError::CannotServeTopic`].
    pub fn serves_topic(&self, topic: &TopicDocument) -> Result<(), ExecutorOfferError> {
        if !self.is_open() {
            return Err(ExecutorOfferError::Closed);
        }
        if let Some(need) = topic.eval_executor.require_offer_commitment.as_deref() {
            if !need.trim().eq_ignore_ascii_case(&self.config_commitment) {
                return Err(ExecutorOfferError::CannotServeTopic);
            }
        }
        Ok(())
    }

    /// Proof deadline for `topic` on this executor: the offer deadline,
    /// tightened by the topic when it names a shorter one.
    pub fn effective_deadline_s(&self, topic: &TopicDocument) -> u64 {
        topic
            .eval_executor
            .max_proof_deadline_s
            .map_or(self.max_proof_deadline_s, |t| {
                t.min(self.max_proof_deadline_s)
            })
    }
}

/// Fail-closed readiness: missing / closed / invalid offer cannot score.
///
/// # Errors
///
/// [`ExecutorOfferError::Missing`], [`ExecutorOfferError::Closed`], or a
/// validate error.
pub fn require_open_executor<'a>(
    offer: Option<&'a EvalExecutorOffer>,
    pin: &ProofPin,
) -> Result<&'a EvalExecutorOffer, ExecutorOfferError> {
    let offer = offer.ok_or(ExecutorOfferError::Missing)?;
    offer.validate(pin)?;
    if !offer.is_open() {
        return Err(ExecutorOfferError::Closed);
    }
    Ok(offer)
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    pub const DIGEST_HEX: &str = "78b614a1f51ce5dd80076c4e343a2b31b85d6c36025e02836cb83929867e7009";

    pub fn pin() -> ProofPin {
        ProofPin {
            eval_image_digest: format!("sha256:{DIGEST_HEX}"),
            topic_pubkey: "ab".repeat(32),
            allowed_lium_template_prefixes: vec!["proof-eval-".into()],
            ..ProofPin::default()
        }
    }

    pub fn offer_for(template_id: &str, deadline: u64, pin: &ProofPin) -> EvalExecutorOffer {
        let mut o = EvalExecutorOffer {
            offer_id: "lium-1x-v0".into(),
            lium_template_id: template_id.into(),
            machine_shape: "1x".into(),
            max_proof_deadline_s: deadline,
            eval_image_digest: pin.eval_image_digest.clone(),
            config_commitment: String::new(),
            status: OfferStatus::Open,
        };
        o.config_commitment = o.expected_commitment();
        o
    }

    pub fn offer() -> EvalExecutorOffer {
        offer_for("proof-eval-78b614a1f51c", 7_200, &pin())
    }
}

#[cfg(test)]
mod tests {
    use proof_task::TopicEvalExecutor;

    use super::fixtures::{offer, offer_for, pin};
    use super::*;

    #[test]
    fn a_well_formed_open_offer_validates() {
        let o = offer();
        o.validate(&pin()).expect("valid");
        assert!(o.is_open());
        assert_eq!(o.gpu_count(), Some(1));
        require_open_executor(Some(&o), &pin()).expect("ready");
    }

    #[test]
    fn json_round_trip_and_unknown_key_refused() {
        let o = offer();
        let body = serde_json::to_string(&o).expect("json");
        let back = EvalExecutorOffer::from_json(&body).expect("parse");
        assert_eq!(back, o);
        back.validate(&pin()).expect("valid after round trip");
        let err = EvalExecutorOffer::from_json(r#"{"offer_id":"x","lium_api_key":"nope"}"#)
            .expect_err("unknown key");
        assert!(err.to_string().contains("lium_api_key"), "{err}");
    }

    #[test]
    fn commitment_is_stable_and_binds_every_knob() {
        let a = executor_config_commitment("proof-eval-78b614a1f51c", "1x", 7_200, "sha256:aa");
        let b = executor_config_commitment("proof-eval-78b614a1f51c", "1x", 7_200, "sha256:aa");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        for other in [
            executor_config_commitment("proof-eval-000000000000", "1x", 7_200, "sha256:aa"),
            executor_config_commitment("proof-eval-78b614a1f51c", "8x", 7_200, "sha256:aa"),
            executor_config_commitment("proof-eval-78b614a1f51c", "1x", 3_600, "sha256:aa"),
            executor_config_commitment("proof-eval-78b614a1f51c", "1x", 7_200, ""),
        ] {
            assert_ne!(a, other);
        }
        let mut o = offer();
        o.config_commitment = "cd".repeat(32);
        assert!(matches!(
            o.validate(&pin()),
            Err(ExecutorOfferError::CommitmentMismatch)
        ));
        o.config_commitment = "not-hex".into();
        assert!(matches!(
            o.validate(&pin()),
            Err(ExecutorOfferError::CommitmentMismatch)
        ));
    }

    #[test]
    fn missing_or_closed_offer_cannot_score() {
        assert!(matches!(
            require_open_executor(None, &pin()),
            Err(ExecutorOfferError::Missing)
        ));
        let mut closed = offer();
        closed.status = OfferStatus::Closed;
        closed.validate(&pin()).expect("closed may load");
        assert!(matches!(
            require_open_executor(Some(&closed), &pin()),
            Err(ExecutorOfferError::Closed)
        ));
        assert!(matches!(
            closed.serves_topic(&TopicDocument::default()),
            Err(ExecutorOfferError::Closed)
        ));
    }

    #[test]
    fn any_shape_but_the_pin_gpu_class_is_refused() {
        for shape in ["8x", "2x", "1", "", "1X"] {
            let mut o = offer();
            o.machine_shape = shape.into();
            o.config_commitment = o.expected_commitment();
            assert!(
                matches!(
                    o.validate(&pin()),
                    Err(ExecutorOfferError::ShapeMismatch { .. })
                ),
                "{shape:?} must not score on a 1x pin"
            );
        }
        assert_eq!(shape_gpu_count("1x"), Some(1));
        assert_eq!(shape_gpu_count("8x"), Some(8));
        assert_eq!(shape_gpu_count("0x"), None);
        assert_eq!(shape_gpu_count("x"), None);
        assert_eq!(shape_gpu_count("b200"), None);
    }

    #[test]
    fn deadline_cannot_loosen_the_pin_ceiling() {
        let p = pin();
        let over = offer_for(
            "proof-eval-78b614a1f51c",
            p.max_proof_deadline_s_ceiling + 1,
            &p,
        );
        assert!(matches!(
            over.validate(&p),
            Err(ExecutorOfferError::BadDeadline(7_201, 7_200))
        ));
        let zero = offer_for("proof-eval-78b614a1f51c", 0, &p);
        assert!(matches!(
            zero.validate(&p),
            Err(ExecutorOfferError::BadDeadline(0, 7_200))
        ));
        let mut tight = p.clone();
        tight.max_proof_deadline_s_ceiling = 3_600;
        assert!(matches!(
            offer().validate(&tight),
            Err(ExecutorOfferError::BadDeadline(7_200, 3_600))
        ));
        offer_for("proof-eval-78b614a1f51c", 3_600, &tight)
            .validate(&tight)
            .expect("at the tightened ceiling");
    }

    #[test]
    fn digest_must_match_the_pin_when_present() {
        let p = pin();
        let mut o = offer();
        o.eval_image_digest = format!("sha256:{}", "ab".repeat(32));
        o.config_commitment = o.expected_commitment();
        assert!(matches!(
            o.validate(&p),
            Err(ExecutorOfferError::DigestMismatch)
        ));
        o.eval_image_digest = String::new();
        o.config_commitment = o.expected_commitment();
        o.validate(&p).expect("unbound digest is legal");
        o.eval_image_digest = p.eval_image_digest.to_ascii_uppercase();
        o.config_commitment = o.expected_commitment();
        o.validate(&p).expect("case-insensitive digest");
    }

    #[test]
    fn template_id_obeys_the_pin_allowlist_and_the_pinned_digest() {
        let p = pin();
        assert!(matches!(
            offer_for("prism-recipe-v10", 600, &p).validate(&p),
            Err(ExecutorOfferError::TemplateNotAllowed(_))
        ));
        assert!(matches!(
            offer_for("proof-eval-000000000000", 600, &p).validate(&p),
            Err(ExecutorOfferError::TemplateDigestMismatch(..))
        ));
        for bad in [
            "",
            "has space",
            "tab\tid",
            &"a".repeat(MAX_TEMPLATE_ID_LEN + 1),
        ] {
            let mut o = offer();
            o.lium_template_id = bad.to_owned();
            o.config_commitment = o.expected_commitment();
            assert!(
                matches!(o.validate(&p), Err(ExecutorOfferError::BadTemplateId)),
                "{bad:?}"
            );
        }
        // A raw Lium UUID is legal only when the pin allowlist admits it.
        let uuid = "f2f5e84c-3b09-4090-be83-1913eabd009e";
        assert!(is_lium_template_uuid(uuid));
        assert!(!is_lium_template_uuid("proof-eval-78b614a1f51c"));
        assert!(!is_lium_template_uuid(
            "f2f5e84c-3b09-4090-be83-1913eabd009"
        ));
        assert!(matches!(
            offer_for(uuid, 600, &p).validate(&p),
            Err(ExecutorOfferError::TemplateNotAllowed(_))
        ));
        let mut open = p.clone();
        open.allowed_lium_template_prefixes.clear();
        offer_for(uuid, 600, &open)
            .validate(&open)
            .expect("uuid under an empty allowlist");
        // Unpinned digest (pre-launch): the name cannot be checked against
        // a digest, and the host cannot rent anyway.
        let mut unpinned = p.clone();
        unpinned.eval_image_digest.clear();
        offer_for("proof-eval-deadbeef0000", 600, &unpinned)
            .validate(&unpinned)
            .expect("pre-launch pin skips the digest-name check");
    }

    #[test]
    fn bad_offer_id_is_refused() {
        let mut o = offer();
        o.offer_id = "Bad Id".into();
        assert!(matches!(
            o.validate(&pin()),
            Err(ExecutorOfferError::BadId(_))
        ));
    }

    #[test]
    fn public_view_shows_every_field_and_no_paths() {
        let v = offer().public_view();
        assert_eq!(v["offer_id"], "lium-1x-v0");
        assert_eq!(v["lium_template_id"], "proof-eval-78b614a1f51c");
        assert_eq!(v["machine_shape"], "1x");
        assert_eq!(v["gpu_count"], 1);
        assert_eq!(v["max_proof_deadline_s"], 7_200);
        assert_eq!(v["status"], "open");
        assert!(v["config_commitment"]
            .as_str()
            .is_some_and(|c| c.len() == 64));
        let dump = v.to_string();
        assert!(!dump.contains("/run/base"), "{dump}");
        assert!(!dump.contains("api_key"), "{dump}");
    }

    #[test]
    fn topic_commitment_pin_and_deadline_tighten() {
        let o = offer();
        let mut topic = TopicDocument::default();
        o.serves_topic(&topic).expect("no constraints");
        assert_eq!(o.effective_deadline_s(&topic), 7_200);
        topic.eval_executor = TopicEvalExecutor {
            require_offer_commitment: Some(o.config_commitment.to_ascii_uppercase()),
            max_proof_deadline_s: Some(1_800),
        };
        o.serves_topic(&topic).expect("matching commitment");
        assert_eq!(o.effective_deadline_s(&topic), 1_800);
        topic.eval_executor.max_proof_deadline_s = Some(9_999);
        assert_eq!(
            o.effective_deadline_s(&topic),
            7_200,
            "a topic never loosens the offer deadline"
        );
        topic.eval_executor.require_offer_commitment = Some("ab".repeat(32));
        assert!(matches!(
            o.serves_topic(&topic),
            Err(ExecutorOfferError::CannotServeTopic)
        ));
    }
}
