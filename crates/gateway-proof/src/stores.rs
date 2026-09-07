use std::sync::Arc;

use bundle::{EpochBundleV1, ScoreOrAbsence};
use gateway::{
    BundleStore, ChallengesBody, RawWeightRow, RawWeightStore, SealError, SealParams, StoreError,
    Stores,
};
use sha2::{Digest, Sha256};
use weights_api::SealRecord;

use crate::{head, seal::current_seal, Error, Receiver};

pub(crate) fn wrap(receiver: Arc<Receiver>) -> Stores {
    (
        Arc::new(Weights(receiver.clone())),
        Arc::new(Bundles(receiver)),
    )
}

struct Weights(Arc<Receiver>);
struct Bundles(Arc<Receiver>);

impl RawWeightStore for Weights {
    fn insert(&self, row: RawWeightRow) -> Result<RawWeightRow, StoreError> {
        if row.challenge_id == "proof" {
            return Err(StoreError::Backend(
                "Proof requires a v2 round batch".into(),
            ));
        }
        self.0.legacy.0.insert(row)
    }

    fn get(&self, challenge: &str, epoch: u64, miner: &str) -> Option<RawWeightRow> {
        if challenge != "proof" {
            return self.0.legacy.0.get(challenge, epoch, miner);
        }
        self.list_for_epoch(epoch)
            .into_iter()
            .find(|r| r.challenge_id == challenge && r.miner_hotkey == miner)
    }

    fn len(&self) -> usize {
        // This store is a current-round view rather than the legacy historical table.
        let result = self.0.exec.run(|pool| async move {
            let mut conn = pool.acquire().await?;
            Ok(head(&mut conn).await?.map(|d| d.chain_epoch))
        });
        visible(result)
            .flatten()
            .map_or(0, |epoch| self.list_for_epoch(epoch).len())
    }

    fn list_for_epoch(&self, epoch: u64) -> Vec<RawWeightRow> {
        let public = self.0.config.proof_public_key;
        let proof = self.0.exec.run(move |pool| async move {
            let mut conn = pool.acquire().await?;
            let document = head(&mut conn).await?.ok_or(Error::Conflict)?;
            if document.chain_epoch != epoch {
                return Err(Error::Conflict);
            }
            Ok(document.verify(&public)?)
        });
        let Some(proof) = visible(proof) else {
            return Vec::new();
        };
        let mut rows = self.0.legacy.0.list_for_epoch(epoch);
        rows.retain(|r| r.challenge_id != "proof");
        rows.extend(proof.into_iter().map(|l| {
            let payload = bundle::raw_weight_payload(
                &l.challenge_id,
                &l.miner_hotkey,
                l.epoch,
                &l.score_or_absence,
            );
            let (kind, score, absence_reason) = match l.score_or_absence {
                ScoreOrAbsence::Score { value } => ("score".to_owned(), Some(value), None),
                ScoreOrAbsence::NoScore { reason } => (
                    "no_score".to_owned(),
                    None,
                    Some((reason as u8).to_string()),
                ),
            };
            RawWeightRow {
                // This is a read-only projection, not a legacy raw-row identity.
                id: uuid::Uuid::nil(),
                challenge_id: "proof".into(),
                epoch: l.epoch,
                miner_hotkey: hex::encode(l.miner_hotkey),
                kind,
                score,
                absence_reason,
                payload_digest: Sha256::digest(&payload).into(),
                payload,
                challenge_sig: l.challenge_sig.to_vec(),
            }
        }));
        rows
    }
}

impl BundleStore for Bundles {
    fn seal_override(
        &self,
        challenges: &ChallengesBody,
        params: &SealParams,
    ) -> Option<Result<EpochBundleV1, SealError>> {
        Some(if *challenges == self.0.challenges {
            self.0
                .seal(params)
                .map_err(|e| SealError::Bundle(e.to_string()))
        } else {
            Err(SealError::Bundle("Proof trust root changed".into()))
        })
    }

    fn pinned_block(&self, epoch: u64) -> Option<Result<u64, SealError>> {
        let result = self.0.exec.run(move |pool| async move {
            let mut conn = pool.acquire().await?;
            let document = head(&mut conn).await?.ok_or(Error::Conflict)?;
            if document.chain_epoch != epoch {
                return Err(Error::Conflict);
            }
            Ok(document.block)
        });
        Some(result.map_err(|e| SealError::Bundle(e.to_string())))
    }

    // Legacy's infallible persistence methods must never acknowledge a v2 write.
    fn put_if_absent(&self, _epoch: u64, _bytes: Vec<u8>) -> Vec<u8> {
        Vec::new()
    }
    fn put_revision(&self, _epoch: u64, _bytes: Vec<u8>) -> Vec<u8> {
        Vec::new()
    }

    fn get_by_epoch(&self, epoch: u64) -> Option<Vec<u8>> {
        let (bytes, _) = self.latest_sealed()?;
        (EpochBundleV1::decode_bytes(&bytes).ok()?.body.epoch == epoch).then_some(bytes)
    }

    fn get_by_root(&self, root: &[u8; 32]) -> Option<Vec<u8>> {
        let (bytes, _) = self.latest_sealed()?;
        (EpochBundleV1::decode_bytes(&bytes).ok()?.body.merkle_root == *root).then_some(bytes)
    }

    fn latest_epoch(&self) -> Option<u64> {
        Some(
            EpochBundleV1::decode_bytes(&self.latest_sealed()?.0)
                .ok()?
                .body
                .epoch,
        )
    }

    fn seal_record(&self, epoch: u64) -> Option<SealRecord> {
        let (bytes, record) = self.latest_sealed()?;
        (EpochBundleV1::decode_bytes(&bytes).ok()?.body.epoch == epoch).then_some(record)
    }

    fn latest_sealed(&self) -> Option<(Vec<u8>, SealRecord)> {
        visible(self.0.exec.run(|pool| async move {
            let mut conn = pool.acquire().await?;
            current_seal(&mut conn).await
        }))
        .flatten()
    }
}

fn visible<T>(result: Result<T, Error>) -> Option<T> {
    match result {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!(error = %e, "Proof gateway read failed closed");
            None
        }
    }
}
