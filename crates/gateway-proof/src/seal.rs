use std::sync::Arc;

use bundle::{EpochBundleV1, LeafV1, LocalTrustRoot, RawWeightBodyV1};
use gateway::SealParams;
use parity_scale_codec::{DecodeAll, Encode};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, Row};
use weights_api::SealRecord;

use crate::{head, integer, lock, Error, Receiver};

impl Receiver {
    pub(crate) fn seal(self: &Arc<Self>, params: &SealParams) -> Result<EpochBundleV1, Error> {
        let params = params.clone();
        let receiver = self.clone();
        self.exec.run(move |pool| async move {
            let mut tx = pool.begin().await?;
            lock(&mut tx).await?;
            let document = head(&mut tx).await?.ok_or(Error::Conflict)?;
            if document.chain_epoch != params.epoch || document.netuid != params.netuid
                || document.block != params.block_b
            {
                return Err(Error::Conflict);
            }
            let mut leaves = document.verify(&receiver.config.proof_public_key)?;
            // One DB snapshot of the other challenges. Never mix cached v1 Proof.
            for row in sqlx::query(
                "SELECT challenge_id, miner_hotkey, payload, signature FROM raw_weight_snapshot \
                 WHERE epoch = $1 AND challenge_id <> 'proof' ORDER BY challenge_id, miner_hotkey",
            ).bind(integer(params.epoch)?).fetch_all(&mut *tx).await? {
                let payload: Vec<u8> = row.try_get("payload")?;
                let body = RawWeightBodyV1::decode_all(&mut payload.as_slice())
                    .map_err(|_| Error::Invalid)?;
                if body.encode() != payload || body.epoch != params.epoch
                    || body.challenge_id != row.try_get::<String, _>("challenge_id")?.as_bytes()
                    || hex::encode(body.miner_hotkey) != row.try_get::<String, _>("miner_hotkey")?
                {
                    return Err(Error::Invalid);
                }
                let entry = receiver.challenges.get(&body.challenge_id).ok_or(Error::Invalid)?;
                if entry.emission_share_bps == 0 { continue; }
                let leaf = LeafV1 {
                    challenge_id: body.challenge_id,
                    miner_hotkey: body.miner_hotkey,
                    epoch: body.epoch,
                    score_or_absence: body.score_or_absence,
                    challenge_sig: row.try_get::<Vec<u8>, _>("signature")?
                        .try_into().map_err(|_| Error::Invalid)?,
                };
                challenge_common::verify_leaf_sig(&leaf, &entry.public_key)
                    .map_err(|_| Error::Unauthorized)?;
                leaves.push(leaf);
            }
            let round = integer(document.round)?;
            // The DB fence remains held across chain validation, signing and commit.
            let bundle = tokio::task::spawn_blocking(move || {
                let snapshot = receiver.validate_chain(&document)?;
                let bundle = bundle::build_sealed_bundle(
                    receiver.chain.as_ref(),
                    &LocalTrustRoot {
                        challenges: receiver.challenges.clone(),
                        measurements_digest: params.measurements_digest,
                    },
                    leaves,
                    &bundle::SealParams {
                        epoch: params.epoch, netuid: params.netuid,
                        block_b: params.block_b, gateway_secret: params.gateway_secret,
                    },
                ).map_err(|_| Error::Invalid)?;
                let mut roster: Vec<([u8; 32], u16)> = snapshot.metagraph.hotkeys.iter()
                    .enumerate().map(|(uid, key)| {
                        Ok((key.as_slice().try_into().map_err(|_| Error::Invalid)?,
                            u16::try_from(uid).map_err(|_| Error::Invalid)?))
                    }).collect::<Result<_, Error>>()?;
                roster.sort_unstable();
                if bundle.body.block_hash != snapshot.hash || bundle.body.uid_map != roster {
                    return Err(Error::Invalid);
                }
                Ok(bundle)
            }).await.map_err(|_| Error::Unavailable)??;
            if let Some((existing, _)) = current_seal(&mut tx).await? {
                let previous = EpochBundleV1::decode_bytes(&existing).map_err(|_| Error::Invalid)?;
                if previous.body == bundle.body {
                    tx.commit().await?;
                    return Ok(previous);
                }
            }
            let body = &bundle.body;
            let epoch = integer(body.epoch)?;
            let revision: i32 = sqlx::query_scalar(
                "INSERT INTO epoch_bundle (epoch, revision, protocol_version, block_number, \
                 block_hash, metagraph_root, merkle_root, measurements_digest, vector_hash, payload, signature) \
                 SELECT $1, COALESCE((SELECT MAX(revision) FROM epoch_bundle WHERE epoch = $1), 0) + 1, \
                 $2, $3, $4, $5, $6, $7, $8, $9, $10 RETURNING revision",
            ).bind(epoch).bind(i32::from(body.protocol_version)).bind(integer(body.block_b)?)
                .bind(body.block_hash.as_slice()).bind(body.metagraph_root.as_slice())
                .bind(body.merkle_root.as_slice()).bind(body.measurements_digest.as_slice())
                .bind(Sha256::digest(body.final_vector.encode()).as_slice())
                .bind(bundle.encode_bytes()).bind(bundle.gateway_sig.as_slice())
                .fetch_one(&mut *tx).await?;
            sqlx::query("INSERT INTO gateway_proof_seal (round, epoch, revision) VALUES ($1, $2, $3)")
                .bind(round).bind(epoch).bind(revision).execute(&mut *tx).await?;
            let (readback, _) = current_seal(&mut tx).await?.ok_or(Error::Unavailable)?;
            if readback != bundle.encode_bytes() { return Err(Error::Unavailable); }
            // A canceled/failed COMMIT is never reported as a successful seal.
            tx.commit().await?;
            Ok(bundle)
        })
    }
}

pub(crate) async fn current_seal(
    conn: &mut PgConnection,
) -> Result<Option<(Vec<u8>, SealRecord)>, Error> {
    let row = sqlx::query(
        "SELECT b.payload, b.revision, (EXTRACT(EPOCH FROM b.created_at) * 1000000)::bigint AS at \
         FROM gateway_proof_seal s JOIN epoch_bundle b USING (epoch, revision) \
         WHERE s.round = (SELECT MAX(round) FROM gateway_proof_round) \
         ORDER BY b.revision DESC LIMIT 1",
    )
    .fetch_optional(conn)
    .await?;
    row.map(|r| {
        Ok((
            r.try_get("payload")?,
            SealRecord {
                revision: u32::try_from(r.try_get::<i32, _>("revision")?)
                    .map_err(|_| Error::Invalid)?,
                sealed_at_micros: u64::try_from(r.try_get::<i64, _>("at")?)
                    .map_err(|_| Error::Invalid)?,
            },
        ))
    })
    .transpose()
}
