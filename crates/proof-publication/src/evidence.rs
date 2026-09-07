//! Strict Cortex-owned evidence publication: the public allowlist document,
//! signed once by the Proof key, delivered by exact bytes and read back.

use async_trait::async_trait;
use parity_scale_codec::{Decode, DecodeAll, Encode};
use proof_autonomy::{commitment, is_digest};
use proof_research::{EvidencePublisher, PublicEvidence, ResearchError};
use serde::{Deserialize, Serialize};

use crate::{limited, pinned_http, Error};

pub const EVIDENCE_ROUTE: &str = "/v2/evidence/proof";
pub const MAX_EVIDENCE_WIRE_BYTES: usize = 4 * 1024;
const DOMAIN: crypto::DomainTag = crypto::DomainTag::new(b"base-proof-evidence-v1");

/// SCALE field order is the wire contract. Floats travel as IEEE-754 bits so the
/// document round-trips byte-identically; the receipt digest is canonical JSON
/// of the reconstructed `PublicEvidence`, matching `ResearchStore`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
#[serde(deny_unknown_fields)]
pub struct EvidencePublication {
    pub schema_version: u32,
    pub evidence_digest: String,
    pub recipe_digest: String,
    pub repetitions: u32,
    pub primary_mean_bits: u64,
    pub primary_standard_error_bits: u64,
    pub passed: bool,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceReceipt {
    pub evidence_digest: String,
    pub digest: String,
}

impl EvidencePublication {
    #[must_use]
    pub fn unsigned(document: &PublicEvidence) -> Self {
        Self {
            schema_version: document.schema_version,
            evidence_digest: document.evidence_digest.clone(),
            recipe_digest: document.recipe_digest.clone(),
            repetitions: document.repetitions,
            primary_mean_bits: document.primary_mean.to_bits(),
            primary_standard_error_bits: document.primary_standard_error.to_bits(),
            passed: document.passed,
            signature: String::new(),
        }
    }

    #[must_use]
    pub fn document(&self) -> PublicEvidence {
        PublicEvidence {
            schema_version: self.schema_version,
            evidence_digest: self.evidence_digest.clone(),
            recipe_digest: self.recipe_digest.clone(),
            repetitions: self.repetitions,
            primary_mean: f64::from_bits(self.primary_mean_bits),
            primary_standard_error: f64::from_bits(self.primary_standard_error_bits),
            passed: self.passed,
        }
    }

    fn signing_digest(&self) -> Result<String, Error> {
        commitment(&self.document()).map_err(|_| Error::Invalid)
    }

    /// # Errors
    /// Unencodable document or invalid secret.
    pub fn sign(&mut self, secret: &[u8; 32]) -> Result<(), Error> {
        self.signature = hex::encode(
            crypto::sign_raw(secret, DOMAIN, self.signing_digest()?.as_bytes())
                .map_err(|_| Error::Unauthorized)?,
        );
        Ok(())
    }

    /// # Errors
    /// Malformed fields, nonfinite statistics or a signature outside the Proof key.
    pub fn verify(&self, public: &[u8; 32]) -> Result<(), Error> {
        let document = self.document();
        if self.encode().len() > MAX_EVIDENCE_WIRE_BYTES
            || self.schema_version != 1
            || !is_digest(&self.evidence_digest)
            || !is_digest(&self.recipe_digest)
            || !(3..=20).contains(&self.repetitions)
            || !document.primary_mean.is_finite()
            || !document.primary_standard_error.is_finite()
            || document.primary_standard_error < 0.0
            || self.signature.len() != 128
            || self.signature != self.signature.to_ascii_lowercase()
        {
            return Err(Error::Invalid);
        }
        let signature: [u8; 64] = hex::decode(&self.signature)
            .map_err(|_| Error::Unauthorized)?
            .try_into()
            .map_err(|_| Error::Unauthorized)?;
        crypto::verify_raw(
            public,
            DOMAIN,
            self.signing_digest()?.as_bytes(),
            &signature,
        )
        .map_err(|_| Error::Unauthorized)
    }

    /// The receipt digest equals `commitment(&PublicEvidence)`, the value
    /// `ResearchStore::publish` compares against its retained summary.
    ///
    /// # Errors
    /// Unencodable document.
    pub fn receipt(&self) -> Result<EvidenceReceipt, Error> {
        Ok(EvidenceReceipt {
            evidence_digest: self.evidence_digest.clone(),
            digest: self.signing_digest()?,
        })
    }

    /// # Errors
    /// Oversized, trailing or noncanonical wire representation.
    pub fn from_wire(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_EVIDENCE_WIRE_BYTES {
            return Err(Error::Invalid);
        }
        let value = Self::decode_all(&mut &bytes[..]).map_err(|_| Error::Invalid)?;
        if value.encode() != bytes {
            return Err(Error::Invalid);
        }
        Ok(value)
    }
}

/// Signs with the operator-held Proof key and confirms only exact readback.
pub struct GatewayEvidencePublisher {
    http: reqwest::Client,
    endpoint: reqwest::Url,
    public: [u8; 32],
    secret: [u8; 32],
}

impl GatewayEvidencePublisher {
    /// # Errors
    /// Invalid destination, HTTP client configuration or signer/pin mismatch.
    pub fn new(base: &str, public: [u8; 32], secret: [u8; 32]) -> Result<Self, Error> {
        if challenge_common::public_key_from_secret(&secret).map_err(|_| Error::Unauthorized)?
            != public
        {
            return Err(Error::Unauthorized);
        }
        let (http, endpoint) = pinned_http(base, EVIDENCE_ROUTE)?;
        Ok(Self {
            http,
            endpoint,
            public,
            secret,
        })
    }

    /// Exact POST receipt AND a readback that verifies under the pinned key
    /// and decodes to exactly the sent document, else unconfirmed. Signatures
    /// are randomized, so a retry after a crash carries new bytes; the receiver
    /// keeps its first accepted bytes for the same content and serves those.
    /// Any HTTP 409 is uncertainty, never confirmation.
    ///
    /// # Errors
    /// Invalid document, HTTP error, conflicting content or mismatched readback.
    pub async fn deliver(&self, document: &PublicEvidence) -> Result<String, Error> {
        let mut publication = EvidencePublication::unsigned(document);
        publication.sign(&self.secret)?;
        publication.verify(&self.public)?;
        let response = self
            .http
            .post(self.endpoint.clone())
            .header("content-type", "application/octet-stream")
            .body(publication.encode())
            .send()
            .await
            .map_err(|_| Error::Unconfirmed)?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(Error::Unconfirmed);
        }
        let receipt: EvidenceReceipt = serde_json::from_slice(&limited(response, 1024).await?)
            .map_err(|_| Error::Unconfirmed)?;
        if receipt != publication.receipt()? {
            return Err(Error::Unconfirmed);
        }
        let mut readback = self.endpoint.clone();
        readback.set_path(&format!("{EVIDENCE_ROUTE}/{}", publication.evidence_digest));
        let response = self
            .http
            .get(readback)
            .header("cache-control", "no-cache, no-store")
            .send()
            .await
            .map_err(|_| Error::Unconfirmed)?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(Error::Unconfirmed);
        }
        let stored =
            EvidencePublication::from_wire(&limited(response, MAX_EVIDENCE_WIRE_BYTES).await?)
                .map_err(|_| Error::Unconfirmed)?;
        stored
            .verify(&self.public)
            .map_err(|_| Error::Unconfirmed)?;
        if stored.document() != publication.document() || stored.receipt()? != receipt {
            return Err(Error::Unconfirmed);
        }
        Ok(receipt.digest)
    }
}

#[async_trait]
impl EvidencePublisher for GatewayEvidencePublisher {
    async fn publish(&self, document: &PublicEvidence) -> Result<String, ResearchError> {
        self.deliver(document)
            .await
            .map_err(|_| ResearchError::Publication)
    }
}
