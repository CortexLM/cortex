//! Versioned contracts outside the untrusted agent and rented GPU.
//!
//! An Atlas judgment is a proposal, not an authenticated measurement. Only
//! the controller can turn admitted evidence and a validated proposal into
//! signed leaves. No types in this crate carry infrastructure credentials.

#![forbid(unsafe_code)]

mod consent;
mod decision;
mod lifecycle;

pub use consent::*;
pub use decision::*;
pub use lifecycle::*;

use serde::Serialize;
use sha2::{Digest, Sha256};

/// Digest of canonical JSON; maps are ordered by `proof_task::canonical_json`.
///
/// # Errors
/// Returns an error when the value cannot be encoded.
pub fn commitment<T: Serialize>(value: &T) -> Result<String, ContractError> {
    let value = serde_json::to_value(value).map_err(|_| ContractError::Encoding)?;
    let canonical = proof_task::canonical_json(&value);
    Ok(hex::encode(Sha256::digest(canonical.as_bytes())))
}

/// Validate a lowercase SHA-256 identifier without accepting aliases.
#[must_use]
pub fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Fail-closed contract failure. Messages never include payloads or secrets.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContractError {
    #[error("invalid contract field: {0}")]
    Invalid(&'static str),
    #[error("contract encoding failed")]
    Encoding,
    #[error("signature verification failed")]
    Signature,
    #[error("expired authorization")]
    Expired,
    #[error("resource or owner outside authorization")]
    Scope,
    #[error("illegal lifecycle transition")]
    Transition,
    #[error("missing or inadmissible evidence")]
    Evidence,
    #[error("stale decision or policy")]
    Stale,
    #[error("allocation exceeds available mass")]
    Allocation,
}
