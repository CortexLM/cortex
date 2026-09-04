//! Master-owned RLM **judge** backend: pin defaults in git, topic tighten,
//! live InferenceOffer off git.
//!
//! The digest-pinned eval image calls this provider to reproduce / cheat-check
//! / score a miner submission. Miners submit claim + code + FLOPs + artifact
//! against a topic; they do **not** train on or bind this offer. Secrets never
//! enter the pin, the topic, or `/v1/status`. No HuggingFace weight bake.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{canonical_json, is_hex64, is_http_origin, ProofPin, TopicDocument, TopicError};

/// Pin `inference_config_schema_version`.
pub const INFERENCE_CONFIG_SCHEMA_VERSION: u32 = 1;

/// Pin `inference_offer_commitment_alg`.
pub const INFERENCE_OFFER_COMMITMENT_ALG: &str = "sha256";

/// Pin / crate ceiling on prompt tokens (covers longctx 32k).
pub const MAX_INPUT_TOKENS_CEILING: u32 = 32_768;

/// Pin / crate ceiling on completion tokens.
pub const MAX_OUTPUT_TOKENS_CEILING: u32 = 8_192;

/// Modes the pin may allow. A pin may subset; it cannot add unknown names.
pub const ALLOWED_MODES: [InferenceMode; 3] = [
    InferenceMode::Chat,
    InferenceMode::Completions,
    InferenceMode::Embeddings,
];

/// Wire name of a scoring / serving mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InferenceMode {
    /// Chat completions.
    Chat,
    /// Text completions.
    Completions,
    /// Embeddings.
    Embeddings,
}

impl InferenceMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Completions => "completions",
            Self::Embeddings => "embeddings",
        }
    }
}

/// How the master exposes the provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InferenceProviderKind {
    /// OpenAI-compatible HTTP (`/v1/chat/completions`, …).
    OpenaiCompatible,
    /// vLLM OpenAI-compatible server.
    Vllm,
    /// Operator-defined HTTP shape the eval image already knows.
    Custom,
}

impl InferenceProviderKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenaiCompatible => "openai_compatible",
            Self::Vllm => "vllm",
            Self::Custom => "custom",
        }
    }
}

/// Live offer lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OfferStatus {
    /// Judge backend is live; required for `can_score`.
    Open,
    /// Host cannot score.
    Closed,
}

/// Provider identity. `base_url` is operator state — never on public status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InferenceProvider {
    /// `openai_compatible` | `vllm` | `custom`.
    pub kind: InferenceProviderKind,
    /// Provider origin the eval image calls. Not a git pin.
    pub base_url: String,
}

/// Public, secret-free knobs hashed into `config_commitment`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InferenceConfig {
    /// Serving mode.
    pub mode: InferenceMode,
    /// Provider model id (not an HF bake into `proof-eval`).
    pub model_ref: String,
    /// Prompt token cap for this offer.
    pub max_input_tokens: u32,
    /// Completion token cap for this offer.
    pub max_output_tokens: u32,
    /// Sampling temperature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// Nucleus sampling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    /// Provider timeout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// Operator live offer (`PROOF_INFERENCE_OFFER_FILE`). Never committed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InferenceOffer {
    /// Immutable slug identifying this judge backend.
    pub offer_id: String,
    /// Provider kind + origin.
    pub provider: InferenceProvider,
    /// Secret-free config.
    pub config: InferenceConfig,
    /// `sha256` hex of canonical JSON of [`InferenceConfig`].
    pub config_commitment: String,
    /// `open` | `closed`.
    pub status: OfferStatus,
}

/// Pin `[inference]` defaults. Empty `model` / `base_url` is pre-launch
/// fail-closed (like an empty digest), not a boot reject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PinInference {
    pub provider: InferenceProviderKind,
    pub base_url: String,
    pub model: String,
    pub mode: InferenceMode,
    pub max_input_tokens: u32,
    pub max_output_tokens: u32,
}

impl Default for PinInference {
    fn default() -> Self {
        Self {
            provider: InferenceProviderKind::OpenaiCompatible,
            base_url: String::new(),
            model: String::new(),
            mode: InferenceMode::Chat,
            max_input_tokens: MAX_INPUT_TOKENS_CEILING,
            max_output_tokens: MAX_OUTPUT_TOKENS_CEILING,
        }
    }
}

impl PinInference {
    pub fn ready_to_score(&self) -> bool {
        !self.model.trim().is_empty()
            && self.max_input_tokens > 0
            && self.max_output_tokens > 0
            && is_http_origin(&self.base_url)
    }
}

/// Topic override. Omitted fields inherit the pin; tokens may only tighten.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TopicInference {
    pub require_judge_offer_commitment: Option<String>,
    pub provider: Option<InferenceProviderKind>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub mode: Option<InferenceMode>,
    pub max_input_tokens: Option<u32>,
    pub max_output_tokens: Option<u32>,
}

/// Topic over pin, then secret-backed / live-offer URL.
pub fn resolve_inference(
    pin: &ProofPin,
    topic: Option<&TopicInference>,
    secret_url: Option<&str>,
    offer: Option<&InferenceOffer>,
) -> PinInference {
    let d = &pin.inference;
    let urls = [
        topic.and_then(|t| t.base_url.as_deref()).unwrap_or(""),
        d.base_url.as_str(),
        secret_url.unwrap_or(""),
        offer.map_or("", |o| o.provider.base_url.as_str()),
    ];
    PinInference {
        provider: topic.and_then(|t| t.provider).unwrap_or(d.provider),
        base_url: urls
            .into_iter()
            .find(|u| is_http_origin(u))
            .unwrap_or("")
            .to_owned(),
        model: topic
            .and_then(|t| t.model.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(d.model.trim())
            .to_owned(),
        mode: topic.and_then(|t| t.mode).unwrap_or(d.mode),
        max_input_tokens: topic
            .and_then(|t| t.max_input_tokens)
            .unwrap_or(d.max_input_tokens),
        max_output_tokens: topic
            .and_then(|t| t.max_output_tokens)
            .unwrap_or(d.max_output_tokens),
    }
}

impl TopicInference {
    /// Pin allowlist, token tighten-only, and usable override shape.
    ///
    /// # Errors
    ///
    /// [`TopicError::InferenceModeNotAllowed`], [`TopicError::InferenceCeiling`],
    /// [`TopicError::IncompleteInference`], or [`TopicError::BadOfferCommitment`].
    pub fn validate(&self, pin: &ProofPin) -> Result<(), TopicError> {
        if let Some(mode) = self.mode {
            if !pin.allows_mode(mode) {
                return Err(TopicError::InferenceModeNotAllowed(mode));
            }
        }
        let pin_in = pin
            .inference
            .max_input_tokens
            .min(pin.max_input_tokens_ceiling);
        let pin_out = pin
            .inference
            .max_output_tokens
            .min(pin.max_output_tokens_ceiling);
        for (field, got, ceiling) in [
            ("max_input_tokens", self.max_input_tokens, pin_in),
            ("max_output_tokens", self.max_output_tokens, pin_out),
        ] {
            if let Some(got) = got {
                if got == 0 || got > ceiling {
                    return Err(TopicError::InferenceCeiling(field, got, ceiling));
                }
            }
        }
        if self.base_url.as_deref().is_some_and(|u| !is_http_origin(u))
            || self
                .model
                .as_deref()
                .is_some_and(|m| m.trim().is_empty() || m.len() > 256)
        {
            return Err(TopicError::IncompleteInference);
        }
        if let Some(need) = self.require_judge_offer_commitment.as_deref() {
            if !is_hex64(need) {
                return Err(TopicError::BadOfferCommitment);
            }
        }
        Ok(())
    }
}

/// Why an offer was refused or cannot score.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum OfferError {
    /// JSON did not parse, or carried an unknown key.
    #[error("parse inference offer: {0}")]
    Parse(String),
    /// `offer_id` is not a slug.
    #[error("offer_id {0:?} must match [a-z0-9][a-z0-9-]{{1,62}}")]
    BadId(String),
    /// Origin missing or not http(s).
    #[error("provider.base_url must be an http(s) origin")]
    BadBaseUrl,
    /// `model_ref` empty.
    #[error("config.model_ref is required")]
    BadModelRef,
    /// Token cap is zero or above the pin ceiling.
    #[error("config.{0} = {1} must be 1..={2}")]
    BadTokenCap(&'static str, u32, u32),
    /// Sampling knob is non-finite or out of range.
    #[error("config.{0} is not a usable sampling value")]
    BadSampling(&'static str),
    /// Mode not in the pin allowlist.
    #[error("config.mode {0:?} is not in the pin allowed_modes")]
    ModeNotAllowed(InferenceMode),
    /// Declared commitment is not 64 hex or does not match the config.
    #[error("config_commitment does not match sha256(canonical config)")]
    CommitmentMismatch,
    /// No offer loaded on this host.
    #[error("inference offer missing; refuse scoring")]
    Missing,
    /// Offer is present but closed.
    #[error("inference offer is closed; refuse scoring")]
    Closed,
    /// Open offer cannot serve this topic.
    #[error("open inference offer cannot serve topic inference constraints")]
    CannotServeTopic,
    /// Resolved pin+topic config is missing model or http(s) origin.
    #[error("resolved inference config is incomplete; refuse scoring")]
    Incomplete,
}

fn is_secret_key(k: &str) -> bool {
    let l = k.to_ascii_lowercase();
    ["secret", "password", "authorization", "bearer"]
        .iter()
        .any(|n| l.contains(n))
        || l.ends_with("api_key")
        || l.ends_with("_token")
        || l == "token"
}

fn strip_secrets(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                if is_secret_key(&k) {
                    continue;
                }
                out.insert(k, strip_secrets(v));
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(strip_secrets).collect())
        }
        other => other,
    }
}

/// `sha256` hex of canonical JSON of `config` with secret keys stripped.
pub fn inference_config_commitment(config: &InferenceConfig) -> String {
    let value = serde_json::to_value(config).unwrap_or(serde_json::Value::Null);
    let body = canonical_json(&strip_secrets(value));
    let mut h = Sha256::new();
    h.update(body.as_bytes());
    hex::encode(h.finalize())
}

impl InferenceOffer {
    /// Parse one operator offer document.
    ///
    /// # Errors
    ///
    /// [`OfferError::Parse`] on malformed JSON or an unknown key.
    pub fn from_json(body: &str) -> Result<Self, OfferError> {
        serde_json::from_str(body).map_err(|e| OfferError::Parse(e.to_string()))
    }

    /// Structural check against the pin. Does not require `open`.
    ///
    /// # Errors
    ///
    /// See [`OfferError`]. A closed-but-valid offer is legal to load.
    pub fn validate(&self, pin: &ProofPin) -> Result<(), OfferError> {
        if !crate::is_slug(&self.offer_id) {
            return Err(OfferError::BadId(self.offer_id.clone()));
        }
        if !is_http_origin(self.provider.base_url.trim()) {
            return Err(OfferError::BadBaseUrl);
        }
        if self.config.model_ref.trim().is_empty() || self.config.model_ref.len() > 256 {
            return Err(OfferError::BadModelRef);
        }
        if !pin.allows_mode(self.config.mode) {
            return Err(OfferError::ModeNotAllowed(self.config.mode));
        }
        let cfg = &self.config;
        for (field, got, ceiling) in [
            (
                "max_input_tokens",
                cfg.max_input_tokens,
                pin.max_input_tokens_ceiling,
            ),
            (
                "max_output_tokens",
                cfg.max_output_tokens,
                pin.max_output_tokens_ceiling,
            ),
        ] {
            if got == 0 || got > ceiling {
                return Err(OfferError::BadTokenCap(field, got, ceiling));
            }
        }
        for (name, v, hi) in [
            ("temperature", cfg.temperature, 2.0),
            ("top_p", cfg.top_p, 1.0),
        ] {
            if v.is_some_and(|x| !x.is_finite() || !(0.0..=hi).contains(&x)) {
                return Err(OfferError::BadSampling(name));
            }
        }
        let want = inference_config_commitment(&self.config);
        if !is_hex64(&self.config_commitment) || !self.config_commitment.eq_ignore_ascii_case(&want)
        {
            return Err(OfferError::CommitmentMismatch);
        }
        Ok(())
    }

    /// Whether this judge backend is open for scoring.
    pub fn is_open(&self) -> bool {
        self.status == OfferStatus::Open
    }

    /// Public status payload (no origin, no secrets).
    pub fn public_view(&self) -> serde_json::Value {
        serde_json::json!({
            "offer_id": self.offer_id,
            "provider_kind": self.provider.kind,
            "mode": self.config.mode,
            "model_ref": self.config.model_ref,
            "max_input_tokens": self.config.max_input_tokens,
            "max_output_tokens": self.config.max_output_tokens,
            "config_commitment": self.config_commitment,
            "status": self.status,
        })
    }

    /// Whether this open judge offer can score `topic` (resolved pin+topic vs offer).
    ///
    /// # Errors
    ///
    /// [`OfferError::Closed`] or [`OfferError::CannotServeTopic`].
    pub fn serves_topic(&self, pin: &ProofPin, topic: &TopicDocument) -> Result<(), OfferError> {
        if !self.is_open() {
            return Err(OfferError::Closed);
        }
        let r = resolve_inference(pin, Some(&topic.inference), None, Some(self));
        if self.provider.kind != r.provider
            || self.config.mode != r.mode
            || self.config.max_input_tokens < r.max_input_tokens
            || self.config.max_output_tokens < r.max_output_tokens
            || (!r.model.is_empty() && r.model != self.config.model_ref)
        {
            return Err(OfferError::CannotServeTopic);
        }
        if let Some(need) = topic.inference.require_judge_offer_commitment.as_deref() {
            if !need.trim().eq_ignore_ascii_case(&self.config_commitment) {
                return Err(OfferError::CannotServeTopic);
            }
        }
        Ok(())
    }
}

/// Fail-closed readiness: missing / closed / invalid offer cannot score.
///
/// # Errors
///
/// [`OfferError::Missing`], [`OfferError::Closed`], or a validate error.
pub fn require_open_offer<'a>(
    offer: Option<&'a InferenceOffer>,
    pin: &ProofPin,
) -> Result<&'a InferenceOffer, OfferError> {
    let offer = offer.ok_or(OfferError::Missing)?;
    offer.validate(pin)?;
    if !offer.is_open() {
        return Err(OfferError::Closed);
    }
    Ok(offer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin() -> ProofPin {
        ProofPin {
            topic_pubkey: "ab".repeat(32),
            ..ProofPin::default()
        }
    }

    fn config() -> InferenceConfig {
        InferenceConfig {
            mode: InferenceMode::Chat,
            model_ref: "master-proxy-v0".into(),
            max_input_tokens: 32_768,
            max_output_tokens: 8_192,
            temperature: Some(0.0),
            top_p: None,
            timeout_ms: None,
        }
    }

    fn offer() -> InferenceOffer {
        let config = config();
        InferenceOffer {
            offer_id: "master-v0".into(),
            provider: InferenceProvider {
                kind: InferenceProviderKind::OpenaiCompatible,
                base_url: "http://127.0.0.1:8000/v1".into(),
            },
            config_commitment: inference_config_commitment(&config),
            config,
            status: OfferStatus::Open,
        }
    }

    #[test]
    fn a_well_formed_open_offer_validates() {
        let o = offer();
        o.validate(&pin()).expect("valid");
        assert!(o.is_open());
        require_open_offer(Some(&o), &pin()).expect("ready");
    }

    #[test]
    fn commitment_is_stable_and_ignores_secret_keys() {
        let a = inference_config_commitment(&config());
        let b = inference_config_commitment(&config());
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        let mut sneaky = serde_json::to_value(config()).expect("json");
        sneaky["api_key"] = serde_json::json!("should-not-hash");
        let stripped = strip_secrets(sneaky);
        assert!(stripped.get("api_key").is_none());
    }

    #[test]
    fn missing_or_closed_offer_cannot_score() {
        assert!(matches!(
            require_open_offer(None, &pin()),
            Err(OfferError::Missing)
        ));
        let mut closed = offer();
        closed.status = OfferStatus::Closed;
        closed.validate(&pin()).expect("closed may load");
        assert!(matches!(
            require_open_offer(Some(&closed), &pin()),
            Err(OfferError::Closed)
        ));
    }

    #[test]
    fn empty_origin_or_model_is_refused() {
        let mut o = offer();
        o.provider.base_url = String::new();
        assert!(matches!(o.validate(&pin()), Err(OfferError::BadBaseUrl)));
        o = offer();
        o.provider.base_url = "file:///weights".into();
        assert!(matches!(o.validate(&pin()), Err(OfferError::BadBaseUrl)));
        o = offer();
        o.config.model_ref = String::new();
        o.config_commitment = inference_config_commitment(&o.config);
        assert!(matches!(o.validate(&pin()), Err(OfferError::BadModelRef)));
    }

    #[test]
    fn token_caps_cannot_loosen_the_pin() {
        let mut o = offer();
        o.config.max_input_tokens = MAX_INPUT_TOKENS_CEILING + 1;
        o.config_commitment = inference_config_commitment(&o.config);
        assert!(matches!(
            o.validate(&pin()),
            Err(OfferError::BadTokenCap(..))
        ));
        o = offer();
        o.config.max_output_tokens = 0;
        o.config_commitment = inference_config_commitment(&o.config);
        assert!(matches!(
            o.validate(&pin()),
            Err(OfferError::BadTokenCap(..))
        ));
    }

    #[test]
    fn a_wrong_commitment_is_refused() {
        let mut o = offer();
        o.config_commitment = "cd".repeat(32);
        assert!(matches!(
            o.validate(&pin()),
            Err(OfferError::CommitmentMismatch)
        ));
    }

    #[test]
    fn public_view_never_leaks_origin() {
        let v = serde_json::to_value(offer().public_view()).expect("json");
        let dump = v.to_string();
        assert!(!dump.contains("127.0.0.1"), "{dump}");
        assert!(!dump.contains("base_url"), "{dump}");
        assert!(!dump.contains("api_key"), "{dump}");
        assert_eq!(v["offer_id"], "master-v0");
        assert_eq!(v["provider_kind"], "openai_compatible");
        assert_eq!(v["status"], "open");
    }

    #[test]
    fn open_offer_must_cover_the_topic() {
        let mut topic = TopicDocument::default();
        topic.inference.mode = Some(InferenceMode::Chat);
        topic.inference.max_input_tokens = Some(4_096);
        topic.inference.max_output_tokens = Some(256);
        offer().serves_topic(&pin(), &topic).expect("covers");
        topic.inference.mode = Some(InferenceMode::Embeddings);
        assert!(matches!(
            offer().serves_topic(&pin(), &topic),
            Err(OfferError::CannotServeTopic)
        ));
        topic.inference.mode = Some(InferenceMode::Chat);
        topic.inference.require_judge_offer_commitment = Some("ab".repeat(32));
        assert!(matches!(
            offer().serves_topic(&pin(), &topic),
            Err(OfferError::CannotServeTopic)
        ));
    }

    #[test]
    fn pin_defaults_resolve_and_topic_may_override_or_tighten() {
        let mut p = pin();
        p.inference.model = "pin-model".into();
        p.inference.max_input_tokens = 4_096;
        p.inference.max_output_tokens = 512;
        let inherited = resolve_inference(&p, None, None, None);
        assert_eq!(inherited.model, "pin-model");
        assert_eq!(inherited.mode, InferenceMode::Chat);
        assert_eq!(inherited.max_input_tokens, 4_096);
        assert!(!inherited.ready_to_score());
        let mut topic = TopicInference {
            model: Some("topic-model".into()),
            max_input_tokens: Some(2_048),
            max_output_tokens: Some(128),
            ..TopicInference::default()
        };
        let over = resolve_inference(&p, Some(&topic), None, Some(&offer()));
        assert_eq!(over.model, "topic-model");
        assert_eq!(over.max_input_tokens, 2_048);
        assert_eq!(over.base_url, "http://127.0.0.1:8000/v1");
        assert!(over.ready_to_score());
        topic.max_input_tokens = Some(8_192);
        assert!(matches!(
            topic.validate(&p),
            Err(crate::TopicError::InferenceCeiling(..))
        ));
    }

    #[test]
    fn missing_model_or_origin_is_incomplete() {
        let p = pin();
        assert!(p.inference.model.is_empty());
        let r = resolve_inference(&p, None, None, Some(&offer()));
        assert!(r.model.trim().is_empty());
        let mut t = TopicInference {
            model: Some("live-model".into()),
            ..TopicInference::default()
        };
        assert!(resolve_inference(&p, Some(&t), None, Some(&offer())).ready_to_score());
        t.base_url = Some("ftp://nope".into());
        assert!(matches!(
            t.validate(&p),
            Err(crate::TopicError::IncompleteInference)
        ));
    }

    #[test]
    fn unknown_offer_key_is_refused() {
        let err =
            InferenceOffer::from_json(r#"{"offer_id":"x","api_key":"nope"}"#).expect_err("unknown");
        assert!(format!("{err}").contains("api_key"), "{err}");
    }
}
