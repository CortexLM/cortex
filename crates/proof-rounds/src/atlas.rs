use async_trait::async_trait;
use proof_autonomy::commitment;
use proof_runtime::{RuntimeCall, RuntimeError, RuntimeOperations, RuntimeScope};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{FrozenRound, RoundError, RoundLease, RoundStore};

/// Controller-side Atlas capability. Keys never enter the agent process.
pub struct AtlasOperations {
    store: RoundStore,
    lease: RoundLease,
    secret: [u8; 32],
    scope: RuntimeScope,
}

impl AtlasOperations {
    /// # Errors
    /// Expired ownership or a signer different from the frozen Proof key.
    pub async fn bind(
        store: RoundStore,
        lease: RoundLease,
        secret: [u8; 32],
    ) -> Result<Self, RoundError> {
        let frozen = store.authorize(&lease).await?;
        let public =
            challenge_common::public_key_from_secret(&secret).map_err(|_| RoundError::Invalid)?;
        if hex::encode(public) != frozen.config.proof_public_key {
            return Err(RoundError::Invalid);
        }
        let scope = RuntimeScope {
            role: "atlas".into(),
            id: lease.round.to_string(),
            commitment: commitment(&frozen)?,
        };
        Ok(Self {
            store,
            lease,
            secret,
            scope,
        })
    }

    #[must_use]
    pub fn scope(&self) -> &RuntimeScope {
        &self.scope
    }

    async fn evidence(
        &self,
        frozen: &FrozenRound,
        input: EvidenceRead,
    ) -> Result<Value, RuntimeError> {
        match input {
            EvidenceRead::Page(page) => {
                page.validate(32)?;
                let items: Vec<_> = frozen
                    .evidence
                    .iter()
                    .skip(page.offset)
                    .take(page.limit)
                    .collect();
                Ok(
                    json!({ "snapshot": frozen.snapshot, "items": items, "total": frozen.evidence.len() }),
                )
            }
            EvidenceRead::Document { evidence_digest } => {
                let expected = frozen
                    .evidence
                    .get(&evidence_digest)
                    .ok_or(RuntimeError::Scope)?;
                let record = self.store.research.adjudication(&evidence_digest).await?;
                if record.summary != *expected {
                    return Err(RuntimeError::Scope);
                }
                Ok(json!({"evidence": record}))
            }
            EvidenceRead::Artifact {
                evidence_digest,
                artifact_digest,
                offset,
                limit,
            } => {
                if !frozen.evidence.contains_key(&evidence_digest) {
                    return Err(RuntimeError::Scope);
                }
                Page { offset, limit }.validate(16 * 1024)?;
                let bytes = self
                    .store
                    .research
                    .retained_artifact(&evidence_digest, &artifact_digest)
                    .await?;
                let end = offset.saturating_add(limit).min(bytes.len());
                let chunk = bytes.get(offset..end).ok_or(RuntimeError::Scope)?;
                Ok(json!({"artifact_digest": artifact_digest, "offset": offset,
                    "total": bytes.len(), "hex": hex::encode(chunk)}))
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum EvidenceRead {
    Page(Page),
    Document {
        evidence_digest: String,
    },
    Artifact {
        evidence_digest: String,
        artifact_digest: String,
        offset: usize,
        limit: usize,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    #[serde(default)]
    offset: usize,
    limit: usize,
}

impl Page {
    fn validate(&self, maximum: usize) -> Result<(), RuntimeError> {
        if !(1..=maximum).contains(&self.limit) {
            return Err(RuntimeError::Scope);
        }
        Ok(())
    }
}

#[async_trait]
impl RuntimeOperations for AtlasOperations {
    async fn call(&self, request: RuntimeCall) -> Result<Value, RuntimeError> {
        if request.schema_version != 1 || request.scope != self.scope {
            return Err(RuntimeError::Scope);
        }
        let frozen = self
            .store
            .authorize(&self.lease)
            .await
            .map_err(|_| RuntimeError::Scope)?;
        let response = match request.operation.as_str() {
            "read_evidence" => {
                self.evidence(&frozen, serde_json::from_value(request.arguments)?)
                    .await?
            }
            "history" => {
                let page: Page = serde_json::from_value(request.arguments)?;
                page.validate(32)?;
                let items: Vec<_> = frozen
                    .contributions
                    .iter()
                    .skip(page.offset)
                    .take(page.limit)
                    .collect();
                json!({ "snapshot": frozen.snapshot, "items": items, "total": frozen.contributions.len(),
                        "history_digest": frozen.history_digest })
            }
            "submit_decision" => {
                let decision = serde_json::from_value(request.arguments)?;
                let stored = self
                    .store
                    .decide(&self.lease, &decision, &self.secret)
                    .await
                    .map_err(|_| RuntimeError::Scope)?;
                json!({ "decision_digest": commitment(&stored.decision).map_err(|_| RuntimeError::Unavailable)?,
                    "publication_pending": true })
            }
            _ => return Err(RuntimeError::Scope),
        };
        self.store
            .authorize(&self.lease)
            .await
            .map_err(|_| RuntimeError::Scope)?;
        Ok(response)
    }
}
