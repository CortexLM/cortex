use proof_autonomy::{commitment, ExperimentState};
use proof_autonomy_pg::{ControllerLease, ControllerTransaction, Experiment, PgStore};
use proof_task::ProofPin;
use sqlx::{types::Json, PgPool, Row};
use uuid::Uuid;

use crate::{
    PublicEvidence, ResearchError, RetainedArtifacts, ScientificEvidence, ScientificRecipe,
};

#[derive(Clone)]
pub struct ResearchStore {
    pub(crate) pool: PgPool,
    orchestration: PgStore,
    pin: ProofPin,
}

impl ResearchStore {
    #[must_use]
    pub fn new(pool: PgPool, pin: ProofPin) -> Self {
        Self {
            orchestration: PgStore::new(pool.clone()),
            pool,
            pin,
        }
    }

    /// The operator trust pin this store validates against.
    #[must_use]
    pub fn pin(&self) -> &ProofPin {
        &self.pin
    }

    /// Trusted recipe registration, tied to the operator's signed topic/pin.
    ///
    /// # Errors
    /// Invalid topic/baseline/recipe, mismatched duplicate or database failure.
    pub async fn register_recipe(
        &self,
        recipe: &ScientificRecipe,
    ) -> Result<String, ResearchError> {
        recipe.validate(&self.pin)?;
        let digest = commitment(recipe)?;
        sqlx::query("INSERT INTO proof_scientific_recipe (digest, recipe) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(&digest).bind(Json(recipe)).execute(&self.pool).await?;
        self.recipe(&digest).await?;
        Ok(digest)
    }

    /// Read and revalidate immutable recipe bytes against the current trust pin.
    ///
    /// # Errors
    /// Missing/corrupt record or invalidated topic signature/pin.
    pub async fn recipe(&self, digest: &str) -> Result<ScientificRecipe, ResearchError> {
        let recipe: Json<ScientificRecipe> =
            sqlx::query_scalar("SELECT recipe FROM proof_scientific_recipe WHERE digest = $1")
                .bind(digest)
                .fetch_optional(&self.pool)
                .await?
                .ok_or(ResearchError::Evidence)?;
        if commitment(&recipe.0)? != digest {
            return Err(ResearchError::Corrupt);
        }
        recipe.validate(&self.pin)?;
        Ok(recipe.0)
    }

    /// Persist controller observations and artifacts atomically with the public
    /// outbox. Never expose this as an agent-authored evidence write.
    ///
    /// # Errors
    /// Failed scientific checks, wrong/expired resource, cancelled experiment,
    /// stale controller, conflicting evidence or database failure.
    pub async fn record(
        &self,
        lease: &ControllerLease,
        evidence: &ScientificEvidence,
        artifacts: &RetainedArtifacts,
    ) -> Result<PublicEvidence, ResearchError> {
        let recipe = self.recipe(&evidence.recipe_digest).await?;
        let summary = evidence.evaluate(&recipe, artifacts)?;
        let mut tx = self.orchestration.controller_transaction(lease).await?;
        let experiment = tx.experiment();
        if experiment.id != evidence.experiment_id
            || experiment.recipe_digest != evidence.recipe_digest
            || !matches!(
                experiment.state,
                ExperimentState::Running | ExperimentState::Collecting
            )
        {
            return Err(ResearchError::Evidence);
        }
        self.require_resource(&mut tx, evidence).await?;
        let digest = &summary.evidence_digest;
        sqlx::query(
            "INSERT INTO proof_scientific_evidence \
             (digest, experiment_id, recipe_digest, evidence, public_summary, public_digest, passed) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT (digest) DO NOTHING",
        ).bind(digest).bind(evidence.experiment_id).bind(&evidence.recipe_digest)
        .bind(Json(evidence)).bind(Json(&summary)).bind(commitment(&summary)?).bind(summary.passed)
        .execute(tx.connection()).await?;
        for (artifact, bytes) in artifacts {
            sqlx::query(
                "INSERT INTO proof_evidence_artifact (evidence_digest, digest, bytes) \
                 VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
            )
            .bind(digest)
            .bind(artifact)
            .bind(bytes)
            .execute(tx.connection())
            .await?;
        }
        sqlx::query(
            "INSERT INTO proof_publication (evidence_digest) VALUES ($1) ON CONFLICT DO NOTHING",
        )
        .bind(digest)
        .execute(tx.connection())
        .await?;
        // Artifact insertion can block. Keep account revocation serialized and
        // check the original resource deadline again after all such waits.
        self.require_resource(&mut tx, evidence).await?;
        tx.commit().await?;
        self.verify_retained(digest).await
    }

    /// Read a miner's evidence from controller-owned storage, not a rented pod.
    ///
    /// # Errors
    /// Wrong owner, missing/corrupted retained data or database failure.
    pub async fn evidence(
        &self,
        experiment: Uuid,
        miner: &str,
    ) -> Result<PublicEvidence, ResearchError> {
        self.orchestration.experiment(experiment, miner).await?;
        let digest: String = sqlx::query_scalar(
            "SELECT digest FROM proof_scientific_evidence WHERE experiment_id = $1",
        )
        .bind(experiment)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(ResearchError::Evidence)?;
        self.verify_retained(&digest).await
    }

    pub(crate) async fn verify_retained(
        &self,
        digest: &str,
    ) -> Result<PublicEvidence, ResearchError> {
        let row = sqlx::query("SELECT * FROM proof_scientific_evidence WHERE digest = $1")
            .bind(digest)
            .fetch_one(&self.pool)
            .await?;
        let evidence: Json<ScientificEvidence> = row.try_get("evidence")?;
        let recipe = self.recipe(&evidence.recipe_digest).await?;
        let artifacts: Vec<(String, Vec<u8>)> = sqlx::query_as(
            "SELECT digest, bytes FROM proof_evidence_artifact WHERE evidence_digest = $1",
        )
        .bind(digest)
        .fetch_all(&self.pool)
        .await?;
        let summary = evidence.evaluate(&recipe, &artifacts.into_iter().collect())?;
        let stored: Json<PublicEvidence> = row.try_get("public_summary")?;
        if summary != stored.0
            || summary.evidence_digest != digest
            || evidence.experiment_id != row.try_get::<Uuid, _>("experiment_id")?
            || evidence.recipe_digest != row.try_get::<String, _>("recipe_digest")?
            || summary.passed != row.try_get::<bool, _>("passed")?
            || commitment(&summary)? != row.try_get::<String, _>("public_digest")?
        {
            return Err(ResearchError::Corrupt);
        }
        let bound: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM proof_experiment WHERE id = $1 AND recipe_digest = $2)",
        )
        .bind(evidence.experiment_id)
        .bind(&evidence.recipe_digest)
        .fetch_one(&self.pool)
        .await?;
        if !bound {
            return Err(ResearchError::Corrupt);
        }
        Ok(summary)
    }

    /// Credit remains blocked until all retained bytes verify and publication
    /// confirms the exact public allowlist document.
    ///
    /// # Errors
    /// Missing, invalid or unsynchronized evidence.
    pub async fn rewardable(&self, digest: &str) -> Result<PublicEvidence, ResearchError> {
        let summary = self.verify_retained(digest).await?;
        let confirmed: Option<String> = sqlx::query_scalar(
            "SELECT p.confirmed_digest FROM proof_publication p \
             JOIN proof_scientific_evidence s ON s.digest = p.evidence_digest \
             JOIN proof_experiment e ON e.id = s.experiment_id \
             WHERE p.evidence_digest = $1 AND p.delivered AND e.state = 'completed' \
             AND NOT EXISTS(SELECT 1 FROM proof_resource r WHERE r.experiment_id = e.id AND r.status <> 'deleted') \
             AND (SELECT count(*) FROM proof_resource r WHERE r.experiment_id = e.id) = 1",
        )
        .bind(digest)
        .fetch_optional(&self.pool)
        .await?
        .flatten();
        if !summary.passed || confirmed != Some(commitment(&summary)?) {
            return Err(ResearchError::Publication);
        }
        Ok(summary)
    }

    /// Complete research only after verified cleanup and synchronized evidence.
    /// A recorded scientific rejection becomes Rejected, never rewardable.
    /// Cancellation is handled by the independent orchestration cleanup path.
    ///
    /// # Errors
    /// Unpublished/corrupt evidence, uncertain rental, incomplete deletion,
    /// cancellation, wrong experiment, stale lease or invalid lifecycle.
    pub async fn complete(
        &self,
        lease: &ControllerLease,
        digest: &str,
    ) -> Result<Experiment, ResearchError> {
        let summary = self.verify_retained(digest).await?;
        let mut tx = self.orchestration.controller_transaction(lease).await?;
        if tx.experiment().state != ExperimentState::Deleting {
            return Err(ResearchError::Evidence);
        }
        let confirmed: Option<String> = sqlx::query_scalar(
            "SELECT p.confirmed_digest FROM proof_publication p \
             JOIN proof_scientific_evidence s ON s.digest = p.evidence_digest \
             WHERE p.evidence_digest = $1 AND s.experiment_id = $2 AND p.delivered \
             FOR SHARE OF p",
        )
        .bind(digest)
        .bind(lease.experiment_id)
        .fetch_optional(tx.connection())
        .await?
        .flatten();
        if confirmed != Some(commitment(&summary)?) {
            return Err(ResearchError::Publication);
        }
        let blocked: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM proof_resource WHERE experiment_id = $1 AND status <> 'deleted') \
             OR EXISTS(SELECT 1 FROM proof_service_intent WHERE experiment_id = $1 \
                AND (kind = 'cancel' OR (kind = 'provision' AND status <> 'completed')))",
        ).bind(lease.experiment_id).fetch_one(tx.connection()).await?;
        if blocked {
            return Err(ResearchError::Evidence);
        }
        let next = if summary.passed {
            ExperimentState::Completed
        } else {
            ExperimentState::Rejected
        };
        tx.advance(next, "cleanup_verified").await?;
        let experiment = tx.experiment().clone();
        tx.commit().await?;
        Ok(experiment)
    }

    async fn require_resource(
        &self,
        tx: &mut ControllerTransaction,
        evidence: &ScientificEvidence,
    ) -> Result<(), ResearchError> {
        let allowed: Option<Uuid> = sqlx::query_scalar(
            "SELECT a.id FROM proof_resource r \
             JOIN proof_miner_account a ON a.id = r.account_id \
             JOIN proof_machine_quote q ON q.id = r.quote_id \
             WHERE r.experiment_id = $1 AND r.resource_id = $2 AND r.status = 'active' \
             AND q.quote->>'image' = $3 AND q.quote->>'image_digest' = $4 \
             AND NOT a.revoked AND r.authorized_until > extract(epoch FROM clock_timestamp()) \
             FOR SHARE OF a, r",
        )
        .bind(evidence.experiment_id)
        .bind(&evidence.resource_id)
        .bind(&self.pin.eval_image)
        .bind(&self.pin.eval_image_digest)
        .fetch_optional(tx.connection())
        .await?;
        if allowed.is_none() {
            return Err(ResearchError::Evidence);
        }
        Ok(())
    }
}
