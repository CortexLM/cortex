use crypto::DomainTag;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{commitment, is_digest, ContractError};

/// New application domain; existing consensus domains remain unchanged.
pub const CONSENT_DOMAIN: DomainTag = DomainTag::new(b"cortex-proof-consent-v1");
pub const ACTION_DOMAIN: DomainTag = DomainTag::new(b"cortex-proof-action-v1");

/// Prices use integer micro-USD. Provider rounding belongs in the quote,
/// before approval, never in a later provisioning fallback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineQuote {
    pub schema_version: u32,
    pub id: Uuid,
    pub experiment_id: Uuid,
    pub miner_hotkey: String,
    pub account_id: Uuid,
    pub recipe_digest: String,
    pub offer_id: String,
    pub gpu_type: String,
    pub gpu_count: u32,
    pub gpu_memory_mib: u64,
    pub ram_mib: u64,
    pub disk_gib: u64,
    pub image: String,
    pub image_digest: String,
    pub hourly_total_microusd: u64,
    pub maximum_total_microusd: u64,
    pub lifetime_seconds: u64,
    pub issued_at: u64,
    pub expires_at: u64,
    /// Binds the provider's executor/template details, not just their names.
    pub provider_fingerprint: String,
}

impl MachineQuote {
    /// Validate every cost and scope constraint before asking for consent.
    ///
    /// # Errors
    /// Invalid identifiers, unpinned image, unbounded cost, or expiry.
    pub fn validate(&self, now: u64) -> Result<(), ContractError> {
        if self.schema_version != 1 || self.id.is_nil() || self.experiment_id.is_nil() {
            return Err(ContractError::Invalid("quote identity"));
        }
        if self.account_id.is_nil()
            || !is_digest(&self.miner_hotkey)
            || !is_digest(&self.recipe_digest)
            || !is_digest(&self.provider_fingerprint)
        {
            return Err(ContractError::Invalid("quote binding"));
        }
        if self.offer_id.is_empty()
            || self.offer_id.len() > 256
            || self.gpu_type.is_empty()
            || self.gpu_count == 0
            || self.gpu_memory_mib == 0
            || self.ram_mib == 0
            || self.disk_gib == 0
        {
            return Err(ContractError::Invalid("machine"));
        }
        if self.image.is_empty()
            || self.image.contains('@')
            || self.image.contains(char::is_whitespace)
            || !self
                .image_digest
                .strip_prefix("sha256:")
                .is_some_and(is_digest)
        {
            return Err(ContractError::Invalid("image pin"));
        }
        if self.issued_at > now || self.expires_at <= now || self.expires_at <= self.issued_at {
            return Err(ContractError::Expired);
        }
        if self.lifetime_seconds == 0 || self.hourly_total_microusd == 0 {
            return Err(ContractError::Invalid("cost bounds"));
        }
        let cost = u128::from(self.hourly_total_microusd)
            .checked_mul(u128::from(self.lifetime_seconds))
            .ok_or(ContractError::Invalid("cost overflow"))?
            .div_ceil(3_600);
        if cost > u128::from(self.maximum_total_microusd) {
            return Err(ContractError::Invalid("total budget"));
        }
        Ok(())
    }
}

/// The database consumes the quote id atomically after this signature check.
/// A valid signature alone does not make an authorization reusable.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedConsent {
    pub quote_digest: String,
    pub signature: String,
}

impl SignedConsent {
    /// Verify exact-quote consent, including the miner identity and expiry.
    ///
    /// # Errors
    /// Invalid quote, commitment mismatch, or invalid signature.
    pub fn verify(&self, quote: &MachineQuote, now: u64) -> Result<(), ContractError> {
        quote.validate(now)?;
        if self.quote_digest != commitment(quote)? {
            return Err(ContractError::Signature);
        }
        verify_signature(
            &quote.miner_hotkey,
            CONSENT_DOMAIN,
            self.quote_digest.as_bytes(),
            &self.signature,
        )
    }
}

/// A signed API action binds method, path, exact body, expiry and a fresh nonce.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedAction {
    pub miner_hotkey: String,
    pub nonce: Uuid,
    pub expires_at: u64,
    pub method: String,
    pub path: String,
    pub body_digest: String,
    pub signature: String,
}

impl SignedAction {
    /// Verify before consuming the nonce in the same transaction as the action.
    ///
    /// # Errors
    /// Invalid identity, request mismatch, expiry or signature.
    pub fn verify(
        &self,
        method: &str,
        path: &str,
        body_digest: &str,
        now: u64,
    ) -> Result<(), ContractError> {
        if self.nonce.is_nil()
            || !is_digest(&self.miner_hotkey)
            || !is_digest(body_digest)
            || self.method != method
            || self.path != path
            || self.body_digest != body_digest
        {
            return Err(ContractError::Scope);
        }
        if self.expires_at <= now || self.expires_at.saturating_sub(now) > 300 {
            return Err(ContractError::Expired);
        }
        let payload = (
            &self.miner_hotkey,
            self.nonce,
            self.expires_at,
            &self.method,
            &self.path,
            &self.body_digest,
        );
        verify_signature(
            &self.miner_hotkey,
            ACTION_DOMAIN,
            commitment(&payload)?.as_bytes(),
            &self.signature,
        )
    }
}

fn verify_signature(
    hotkey: &str,
    domain: DomainTag,
    payload: &[u8],
    signature: &str,
) -> Result<(), ContractError> {
    let public: [u8; 32] = hex::decode(hotkey)
        .map_err(|_| ContractError::Signature)?
        .try_into()
        .map_err(|_| ContractError::Signature)?;
    let sig = hex::decode(signature).map_err(|_| ContractError::Signature)?;
    crypto::verify_raw(&public, domain, payload, &sig).map_err(|_| ContractError::Signature)
}
