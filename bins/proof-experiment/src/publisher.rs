use std::{path::Path, sync::Arc};

use async_trait::async_trait;
use proof_research::{EvidencePublisher, PublicEvidence, ResearchError};
use proof_task::ProofPin;
use serde::Deserialize;

use crate::private::private_bytes;

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Publisher {
    /// Evidence stays unpublished, therefore never rewardable.
    None,
    /// Strict `/v2/evidence/proof` receiver: exact receipt plus byte readback.
    Gateway { gateway_url: String },
}

impl Publisher {
    /// The signer must be the pin's topic key even when nothing is published,
    /// so a misconfigured secret fails at `--check` rather than at first delivery.
    pub fn build(
        &self,
        secret_file: &Path,
        pin: &ProofPin,
    ) -> Result<Arc<dyn EvidencePublisher>, &'static str> {
        let secret = challenge_keys::parse_challenge_secret(&private_bytes(secret_file, 256)?)
            .map_err(|_| "invalid private Proof signing key")?;
        let public = challenge_common::public_key_from_secret(&secret)
            .map_err(|_| "invalid Proof signer")?;
        if pin.topic_pubkey != hex::encode(public) {
            return Err("Proof signer differs from topic trust pin");
        }
        Ok(match self {
            Self::None => Arc::new(Unpublished),
            Self::Gateway { gateway_url } => Arc::new(
                proof_publication::GatewayEvidencePublisher::new(gateway_url, public, secret)
                    .map_err(|_| "invalid evidence gateway destination")?,
            ),
        })
    }
}

/// `publisher: {"kind": "none"}`. Evidence is retained locally and never
/// confirmed as published; unpublished evidence cannot be rewarded.
pub struct Unpublished;

#[async_trait]
impl EvidencePublisher for Unpublished {
    async fn publish(&self, _: &PublicEvidence) -> Result<String, ResearchError> {
        Err(ResearchError::Publication)
    }
}
