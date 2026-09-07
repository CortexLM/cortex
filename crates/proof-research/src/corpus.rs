use proof_autonomy::commitment;
use serde::{Deserialize, Serialize};
use sqlx::{types::Json, Row};

use crate::{
    artifact_digest, PublicEvidence, ResearchError, ResearchStore, ScientificEvidence,
    ScientificRecipe,
};

/// Controller-admitted corpus entry. A changed seed or rental cannot reset the
/// age of the same candidate against the same topic/baseline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorpusEvidence {
    pub contribution_digest: String,
    pub miner_hotkey: [u8; 32],
    pub summary: PublicEvidence,
}

/// Private Atlas input, not the public publication allowlist.
#[derive(Debug, Clone, Serialize)]
pub struct AdjudicationEvidence {
    pub summary: PublicEvidence,
    pub recipe: ScientificRecipe,
    pub observations: ScientificEvidence,
}

impl ResearchStore {
    /// # Errors
    /// Missing, unconfirmed or corrupted controller-owned evidence.
    pub async fn adjudication(&self, digest: &str) -> Result<AdjudicationEvidence, ResearchError> {
        let summary = self.rewardable(digest).await?;
        let recipe = self.recipe(&summary.recipe_digest).await?;
        let observations: Json<ScientificEvidence> =
            sqlx::query_scalar("SELECT evidence FROM proof_scientific_evidence WHERE digest = $1")
                .bind(digest)
                .fetch_one(&self.pool)
                .await?;
        if commitment(&observations.0)? != digest {
            return Err(ResearchError::Corrupt);
        }
        Ok(AdjudicationEvidence {
            summary,
            recipe,
            observations: observations.0,
        })
    }

    /// Controller-only retained bytes. The caller must bind the evidence to its
    /// private round scope and paginate before sending through runtime IPC.
    ///
    /// # Errors
    /// Missing/unrewardable evidence, unbound artifact or corrupted bytes.
    pub async fn retained_artifact(
        &self,
        evidence: &str,
        digest: &str,
    ) -> Result<Vec<u8>, ResearchError> {
        self.rewardable(evidence).await?;
        let bytes: Vec<u8> = sqlx::query_scalar(
            "SELECT bytes FROM proof_evidence_artifact WHERE evidence_digest = $1 AND digest = $2",
        )
        .bind(evidence)
        .bind(digest)
        .fetch_one(&self.pool)
        .await?;
        if artifact_digest(&bytes) != digest {
            return Err(ResearchError::Corrupt);
        }
        Ok(bytes)
    }

    /// Freeze only completed, synchronized, retained evidence no newer than the
    /// boundary's chain epoch. Invalid retained data fails the freeze closed.
    ///
    /// # Errors
    /// Oversized corpus, corrupted evidence/ownership or database failure.
    pub async fn corpus(
        &self,
        chain_epoch: u64,
        cutoff_ms: u64,
    ) -> Result<Vec<CorpusEvidence>, ResearchError> {
        let cutoff = i64::try_from(cutoff_ms).map_err(|_| ResearchError::Evidence)?;
        if chain_epoch == 0 || cutoff <= 0 {
            return Err(ResearchError::Evidence);
        }
        let rows = sqlx::query(
            "SELECT s.digest, s.evidence, e.miner_hotkey FROM proof_scientific_evidence s \
             JOIN proof_experiment e ON e.id = s.experiment_id \
             JOIN proof_publication p ON p.evidence_digest = s.digest \
             WHERE e.state = 'completed' AND s.passed AND p.delivered \
             AND s.created_at <= to_timestamp($1::bigint::double precision / 1000) \
             AND EXISTS (SELECT 1 FROM proof_experiment_event v WHERE v.experiment_id = e.id \
                 AND v.state = 'completed' AND v.created_at <= to_timestamp($1::bigint::double precision / 1000)) \
             ORDER BY s.created_at, s.digest LIMIT 10001",
        ).bind(cutoff).fetch_all(&self.pool).await?;
        if rows.len() > 10_000 {
            return Err(ResearchError::Evidence);
        }
        let mut corpus = Vec::new();
        for row in rows {
            let evidence: Json<ScientificEvidence> = row.try_get("evidence")?;
            if evidence.chain_epoch > chain_epoch {
                continue;
            }
            let summary = self
                .rewardable(&row.try_get::<String, _>("digest")?)
                .await?;
            let recipe = self.recipe(&summary.recipe_digest).await?;
            let hotkey: String = row.try_get("miner_hotkey")?;
            let miner_hotkey = hex::decode(hotkey)
                .map_err(|_| ResearchError::Corrupt)?
                .try_into()
                .map_err(|_| ResearchError::Corrupt)?;
            corpus.push(CorpusEvidence {
                contribution_digest: commitment(&(
                    &recipe.topic.id,
                    &recipe.topic.baseline.metrics_commitment,
                    &recipe.candidate_script_digest,
                ))?,
                miner_hotkey,
                summary,
            });
        }
        Ok(corpus)
    }
}
