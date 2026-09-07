//! V2 authenticates round metadata as well as the exact, once-signed leaf batch.
#![forbid(unsafe_code)]

mod evidence;

use std::{collections::BTreeSet, time::Duration};

use bundle::LeafV1;
use parity_scale_codec::{Decode, DecodeAll, Encode};
use proof_autonomy::{commitment, is_digest, ROUND_BLOCKS};
use serde::{Deserialize, Serialize};

pub use evidence::{
    EvidencePublication, EvidenceReceipt, GatewayEvidencePublisher, EVIDENCE_ROUTE,
    MAX_EVIDENCE_WIRE_BYTES,
};

pub const ROUTE: &str = "/v2/weights/proof/rounds";
pub const MAX_WIRE_BYTES: usize = 8 * 1024 * 1024;
const DOMAIN: crypto::DomainTag = crypto::DomainTag::new(b"base-proof-publication-v2");

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid publication")]
    Invalid,
    #[error("invalid publication signature")]
    Unauthorized,
    #[error("publication not confirmed")]
    Unconfirmed,
}

/// Field order is the v2 SCALE transport contract. Receipt hashes use canonical JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
#[serde(deny_unknown_fields)]
pub struct RoundPublication {
    pub round: u64,
    pub chain_epoch: u64,
    pub netuid: u16,
    pub block: u64,
    pub block_hash: String,
    pub frozen_digest: String,
    pub decision_digest: String,
    pub leaves: Vec<u8>,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub round: u64,
    pub digest: String,
}

impl RoundPublication {
    fn signing_digest(&self) -> Result<String, Error> {
        commitment(&(
            self.round,
            self.chain_epoch,
            self.netuid,
            self.block,
            &self.block_hash,
            &self.frozen_digest,
            &self.decision_digest,
            &self.leaves,
        ))
        .map_err(|_| Error::Invalid)
    }

    /// Sign once, then retain the returned signature with the decision.
    ///
    /// # Errors
    /// Unencodable fields or invalid secret.
    pub fn sign(&mut self, secret: &[u8; 32]) -> Result<(), Error> {
        self.signature = hex::encode(
            crypto::sign_raw(secret, DOMAIN, self.signing_digest()?.as_bytes())
                .map_err(|_| Error::Unauthorized)?,
        );
        Ok(())
    }

    /// # Errors
    /// Malformed fields, trailing/noncanonical SCALE, duplicate/out-of-order leaves,
    /// or a signature outside the configured Proof key.
    pub fn verify(&self, public: &[u8; 32]) -> Result<Vec<LeafV1>, Error> {
        if self.encode().len() > MAX_WIRE_BYTES
            || self.round > i64::MAX.unsigned_abs()
            || self.block > i64::MAX.unsigned_abs()
            || self.chain_epoch == 0
            || self.chain_epoch > i64::MAX.unsigned_abs()
            || !is_digest(&self.block_hash)
            || self.block_hash == "0".repeat(64)
            || !is_digest(&self.frozen_digest)
            || !is_digest(&self.decision_digest)
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
        .map_err(|_| Error::Unauthorized)?;
        let leaves =
            Vec::<LeafV1>::decode_all(&mut self.leaves.as_slice()).map_err(|_| Error::Invalid)?;
        if leaves.is_empty()
            || leaves.len() > 16_384
            || leaves.encode() != self.leaves
            || leaves
                .windows(2)
                .any(|w| w[0].miner_hotkey >= w[1].miner_hotkey)
        {
            return Err(Error::Invalid);
        }
        for leaf in &leaves {
            if leaf.challenge_id != b"proof" || leaf.epoch != self.chain_epoch {
                return Err(Error::Invalid);
            }
            challenge_common::verify_leaf_sig(leaf, public).map_err(|_| Error::Unauthorized)?;
        }
        Ok(leaves)
    }

    /// # Errors
    /// Invalid boundary, roster or signature.
    pub fn validate_roster(
        &self,
        public: &[u8; 32],
        netuid: u16,
        anchor_block: u64,
        roster: &[[u8; 32]],
    ) -> Result<Vec<LeafV1>, Error> {
        let block = self
            .round
            .checked_add(1)
            .and_then(|n| n.checked_mul(ROUND_BLOCKS))
            .and_then(|n| n.checked_add(anchor_block))
            .ok_or(Error::Invalid)?;
        let expected: BTreeSet<_> = roster.iter().copied().collect();
        let leaves = self.verify(public)?;
        if self.netuid != netuid
            || self.block != block
            || expected.len() != roster.len()
            || leaves
                .iter()
                .map(|l| l.miner_hotkey)
                .collect::<BTreeSet<_>>()
                != expected
        {
            return Err(Error::Invalid);
        }
        Ok(leaves)
    }

    /// # Errors
    /// Unencodable document.
    pub fn receipt(&self) -> Result<Receipt, Error> {
        Ok(Receipt {
            round: self.round,
            digest: commitment(self).map_err(|_| Error::Invalid)?,
        })
    }

    /// # Errors
    /// Oversized, trailing or noncanonical wire representation.
    pub fn from_wire(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_WIRE_BYTES {
            return Err(Error::Invalid);
        }
        let value = Self::decode_all(&mut &bytes[..]).map_err(|_| Error::Invalid)?;
        if value.encode() != bytes {
            return Err(Error::Invalid);
        }
        Ok(value)
    }
}

/// Fixed trusted gateway destination; redirects, credentials and remote cleartext
/// are refused. HTTP is allowed only for loopback integration tests/local gateways.
pub struct Client {
    http: reqwest::Client,
    endpoint: reqwest::Url,
    public: [u8; 32],
}

impl Client {
    /// # Errors
    /// Invalid destination or HTTP client configuration.
    pub fn new(base: &str, public: [u8; 32]) -> Result<Self, Error> {
        let (http, endpoint) = pinned_http(base, ROUTE)?;
        Ok(Self {
            http,
            endpoint,
            public,
        })
    }

    /// Exact POST acknowledgement AND byte-for-byte current readback are required.
    /// A timeout or *any* HTTP 409 is uncertainty, never confirmation.
    ///
    /// # Errors
    /// Invalid document, HTTP error, stale/conflicting round or mismatched readback.
    pub async fn publish(&self, document: &RoundPublication) -> Result<String, Error> {
        document.verify(&self.public)?;
        let bytes = document.encode();
        let response = self
            .http
            .post(self.endpoint.clone())
            .header("content-type", "application/octet-stream")
            .body(bytes.clone())
            .send()
            .await
            .map_err(|_| Error::Unconfirmed)?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(Error::Unconfirmed);
        }
        let receipt: Receipt = serde_json::from_slice(&limited(response, 1024).await?)
            .map_err(|_| Error::Unconfirmed)?;
        if receipt != document.receipt()? {
            return Err(Error::Unconfirmed);
        }
        let mut readback = self.endpoint.clone();
        readback.set_path(&format!("{ROUTE}/{}", document.round));
        let response = self
            .http
            .get(readback)
            .header("cache-control", "no-cache, no-store")
            .send()
            .await
            .map_err(|_| Error::Unconfirmed)?;
        if response.status() != reqwest::StatusCode::OK
            || limited(response, MAX_WIRE_BYTES).await? != bytes
        {
            return Err(Error::Unconfirmed);
        }
        Ok(receipt.digest)
    }
}

pub(crate) fn pinned_http(
    base: &str,
    route: &str,
) -> Result<(reqwest::Client, reqwest::Url), Error> {
    let mut endpoint = reqwest::Url::parse(base).map_err(|_| Error::Invalid)?;
    let loopback = endpoint.host_str().is_some_and(|h| {
        h == "localhost"
            || h.trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if !(endpoint.scheme() == "https" || (endpoint.scheme() == "http" && loopback))
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
        || endpoint.path() != "/"
    {
        return Err(Error::Invalid);
    }
    endpoint.set_path(route);
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| Error::Invalid)?;
    Ok((http, endpoint))
}

pub(crate) async fn limited(mut response: reqwest::Response, cap: usize) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| Error::Unconfirmed)? {
        if bytes.len().saturating_add(chunk.len()) > cap {
            return Err(Error::Unconfirmed);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
