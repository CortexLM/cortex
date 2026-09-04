//! Master-owned inference provider: pin ceilings in git, live offer off git.
//!
//! The eval image calls this provider. It does **not** bake HuggingFace
//! weights. Secrets never enter the pin, the topic, or `/v1/status`.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{canonical_json, ProofPin, TopicDocument};

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
    /// Wire name.
    #[must_use]
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
    /// Wire name.
    #[must_use]
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
    /// Accepts binds; required for `can_score`.
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
    /// Immutable slug miners bind to.
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

/// Fields `/v1/status` may emit. No origin, no key, no file path.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PublicInferenceOffer {
    /// Offer slug.
    pub offer_id: String,
    /// Provider kind only.
    pub provider_kind: InferenceProviderKind,
    /// Mode.
    pub mode: InferenceMode,
    /// Provider model id.
    pub model_ref: String,
    /// Offer input cap.
    pub max_input_tokens: u32,
    /// Offer output cap.
    pub max_output_tokens: u32,
    /// Config commitment.
    pub config_commitment: String,
    /// Lifecycle.
    pub status: OfferStatus,
}

/// Topic-side inference constraints. Tighten pin ceilings; never loosen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TopicInference {
    /// When set, the live offer's commitment must equal this hex.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require_offer_commitment: Option<String>,
    /// Mode this topic is scored under.
    pub mode: InferenceMode,
    /// Topic input cap (`<=` pin ceiling).
    pub max_input_tokens: u32,
    /// Topic output cap (`<=` pin ceiling).
    pub max_output_tokens: u32,
}

impl Default for TopicInference {
    fn default() -> Self {
        Self {
            require_offer_commitment: None,
            mode: InferenceMode::Chat,
            max_input_tokens: MAX_INPUT_TOKENS_CEILING,
            max_output_tokens: MAX_OUTPUT_TOKENS_CEILING,
        }
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
    #[error("config.{field} = {got} must be 1..={ceiling}")]
    BadTokenCap {
        /// Which cap.
        field: &'static str,
        /// Offer value.
        got: u32,
        /// Pin ceiling.
        ceiling: u32,
    },
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
    /// Miner bind does not match the open offer.
    #[error("inference_offer_id / config_commitment is not the open offer")]
    Stale,
    /// Open offer cannot serve this topic.
    #[error("open inference offer cannot serve topic inference constraints")]
    CannotServeTopic,
}

fn is_hex64(s: &str) -> bool {
    let t = s.trim();
    t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit())
}

fn is_secret_key(k: &str) -> bool {
    let l = k.to_ascii_lowercase();
    l.contains("secret")
        || l.contains("password")
        || l.contains("authorization")
        || l.contains("bearer")
        || l.ends_with("api_key")
        || l.ends_with("_token")
        || l == "token"
        || l == "api_key"
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
#[must_use]
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
        if !is_offer_id(&self.offer_id) {
            return Err(OfferError::BadId(self.offer_id.clone()));
        }
        let url = self.provider.base_url.trim();
        if url.len() < 8
            || !(url.starts_with("http://") || url.starts_with("https://"))
            || url.contains('\n')
            || url.contains(' ')
        {
            return Err(OfferError::BadBaseUrl);
        }
        if self.config.model_ref.trim().is_empty() || self.config.model_ref.len() > 256 {
            return Err(OfferError::BadModelRef);
        }
        if !pin.allows_mode(self.config.mode) {
            return Err(OfferError::ModeNotAllowed(self.config.mode));
        }
        for (field, got, ceiling) in [
            (
                "max_input_tokens",
                self.config.max_input_tokens,
                pin.max_input_tokens_ceiling,
            ),
            (
                "max_output_tokens",
                self.config.max_output_tokens,
                pin.max_output_tokens_ceiling,
            ),
        ] {
            if got == 0 || got > ceiling {
                return Err(OfferError::BadTokenCap {
                    field,
                    got,
                    ceiling,
                });
            }
        }
        if let Some(t) = self.config.temperature {
            if !t.is_finite() || !(0.0..=2.0).contains(&t) {
                return Err(OfferError::BadSampling("temperature"));
            }
        }
        if let Some(p) = self.config.top_p {
            if !p.is_finite() || !(0.0..=1.0).contains(&p) {
                return Err(OfferError::BadSampling("top_p"));
            }
        }
        let want = inference_config_commitment(&self.config);
        if !is_hex64(&self.config_commitment) || !self.config_commitment.eq_ignore_ascii_case(&want)
        {
            return Err(OfferError::CommitmentMismatch);
        }
        Ok(())
    }

    /// Whether this offer can accept binds.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.status == OfferStatus::Open
    }

    /// Public status payload (no origin, no secrets).
    #[must_use]
    pub fn public_view(&self) -> PublicInferenceOffer {
        PublicInferenceOffer {
            offer_id: self.offer_id.clone(),
            provider_kind: self.provider.kind,
            mode: self.config.mode,
            model_ref: self.config.model_ref.clone(),
            max_input_tokens: self.config.max_input_tokens,
            max_output_tokens: self.config.max_output_tokens,
            config_commitment: self.config_commitment.clone(),
            status: self.status,
        }
    }

    /// Miner bind: both fields must match this open offer.
    ///
    /// # Errors
    ///
    /// [`OfferError::Stale`] on mismatch. Closed is [`OfferError::Closed`].
    pub fn bind_miner(&self, offer_id: &str, commitment: &str) -> Result<(), OfferError> {
        if !self.is_open() {
            return Err(OfferError::Closed);
        }
        if offer_id.trim() != self.offer_id
            || !commitment
                .trim()
                .eq_ignore_ascii_case(&self.config_commitment)
        {
            return Err(OfferError::Stale);
        }
        Ok(())
    }

    /// Whether this open offer can score `topic` (mode + token floors).
    ///
    /// # Errors
    ///
    /// [`OfferError::Closed`] or [`OfferError::CannotServeTopic`].
    pub fn serves_topic(&self, topic: &TopicDocument) -> Result<(), OfferError> {
        if !self.is_open() {
            return Err(OfferError::Closed);
        }
        let inf = &topic.inference;
        if self.config.mode != inf.mode
            || self.config.max_input_tokens < inf.max_input_tokens
            || self.config.max_output_tokens < inf.max_output_tokens
        {
            return Err(OfferError::CannotServeTopic);
        }
        if let Some(need) = inf.require_offer_commitment.as_deref().map(str::trim) {
            if !need.eq_ignore_ascii_case(&self.config_commitment) {
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

fn is_offer_id(id: &str) -> bool {
    let len = id.len();
    if !(2..=63).contains(&len) {
        return false;
    }
    let mut chars = id.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
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
            Err(OfferError::BadTokenCap { .. })
        ));
        o = offer();
        o.config.max_output_tokens = 0;
        o.config_commitment = inference_config_commitment(&o.config);
        assert!(matches!(
            o.validate(&pin()),
            Err(OfferError::BadTokenCap { .. })
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
    fn miner_bind_is_exact_and_stale_is_named() {
        let o = offer();
        o.bind_miner("master-v0", &o.config_commitment)
            .expect("match");
        assert!(matches!(
            o.bind_miner("other-v0", &o.config_commitment),
            Err(OfferError::Stale)
        ));
        assert!(matches!(
            o.bind_miner("master-v0", &"ab".repeat(32)),
            Err(OfferError::Stale)
        ));
    }

    #[test]
    fn open_offer_must_cover_the_topic() {
        let mut topic = TopicDocument::default();
        topic.inference.mode = InferenceMode::Chat;
        topic.inference.max_input_tokens = 4_096;
        topic.inference.max_output_tokens = 256;
        offer().serves_topic(&topic).expect("covers");
        topic.inference.mode = InferenceMode::Embeddings;
        assert!(matches!(
            offer().serves_topic(&topic),
            Err(OfferError::CannotServeTopic)
        ));
        topic.inference.mode = InferenceMode::Chat;
        topic.inference.require_offer_commitment = Some("ab".repeat(32));
        assert!(matches!(
            offer().serves_topic(&topic),
            Err(OfferError::CannotServeTopic)
        ));
    }

    #[test]
    fn unknown_offer_key_is_refused() {
        let err =
            InferenceOffer::from_json(r#"{"offer_id":"x","api_key":"nope"}"#).expect_err("unknown");
        assert!(format!("{err}").contains("api_key"), "{err}");
    }
}
