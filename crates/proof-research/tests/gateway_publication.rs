#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "../../proof-autonomy-pg/tests/common/mod.rs"]
mod common;
mod science;

use std::sync::Arc;

use chain::{FakeChain, FakeChainConfig};
use common::{Fixture, SEED};
use gateway::{ChallengeEntry, ChallengesBody, ParticipantPolicy};
use gateway_proof::{Config, Error, FinalizedSource, Receiver};
use proof_autonomy::commitment;
use proof_publication::{EvidencePublication, GatewayEvidencePublisher};
use proof_research::ResearchStore;
use science::{clean, running, scientific};

struct NoChain;
impl FinalizedSource for NoChain {
    fn snapshot(&self, _: u64) -> Result<chain_live::FinalizedSnapshot, Error> {
        Err(Error::Unavailable)
    }
}

fn public() -> [u8; 32] {
    challenge_common::public_key_from_secret(&SEED).unwrap()
}

fn receiver(url: &str) -> Arc<Receiver> {
    let chain = FakeChain::new(FakeChainConfig {
        hotkeys: vec![vec![1; 32]],
        ..FakeChainConfig::default()
    });
    let challenges = ChallengesBody {
        challenges: [(b"bounty".to_vec(), 2000), (b"proof".to_vec(), 8000)]
            .into_iter()
            .map(|(id, emission_share_bps)| ChallengeEntry {
                id,
                emission_share_bps,
                public_key: public(),
                policy: ParticipantPolicy::AllMetagraphHotkeys,
            })
            .collect(),
    };
    Receiver::connect(
        url,
        Arc::new(NoChain),
        Arc::new(validator_sync::SyncChain::new(chain)),
        Config {
            netuid: 1,
            anchor_block: 0,
            proof_public_key: public(),
        },
        challenges,
    )
    .unwrap()
}

/// Real store, real signed transport, real durable receiver: publication is
/// confirmed only after the receipt and byte readback match the retained summary.
#[tokio::test]
async fn store_publish_confirms_through_strict_gateway_receiver() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let owner = std::env::var("DATABASE_URL").unwrap();
    let url = format!(
        "{}?options=-c%20search_path%3D{}",
        db::app_role_database_url(&owner).unwrap(),
        f.database.schema()
    );
    let receiver = receiver(&url);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = receiver.router();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let (pin, recipe, evidence, artifacts) = scientific();
    let store = ResearchStore::new(f.database.app_pool().await.unwrap(), pin.clone());
    let digest = store.register_recipe(&recipe).await.unwrap();
    let lease = running(&f, &pin, digest, evidence.experiment_id).await;
    let summary = store.record(&lease, &evidence, &artifacts).await.unwrap();

    // Wrong signer never confirms and leaves the row retryable.
    assert!(
        GatewayEvidencePublisher::new(&format!("http://{address}"), public(), [3; 32]).is_err()
    );
    let publisher =
        GatewayEvidencePublisher::new(&format!("http://{address}"), public(), SEED).unwrap();
    store
        .publish(&summary.evidence_digest, &publisher)
        .await
        .unwrap();
    let confirmed: Option<String> = sqlx::query_scalar(
        "SELECT confirmed_digest FROM proof_publication WHERE evidence_digest = $1 AND delivered",
    )
    .bind(&summary.evidence_digest)
    .fetch_one(f.database.pool())
    .await
    .unwrap();
    assert_eq!(confirmed, Some(commitment(&summary).unwrap()));
    let stored = EvidencePublication::from_wire(
        &receiver
            .readback_evidence(&summary.evidence_digest)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(stored.document(), summary);
    // Idempotent: a second publish is a no-op against the delivered row.
    store
        .publish(&summary.evidence_digest, &publisher)
        .await
        .unwrap();
    clean(&f, &lease).await;
    store
        .complete(&lease, &summary.evidence_digest)
        .await
        .unwrap();
    assert!(
        store
            .rewardable(&summary.evidence_digest)
            .await
            .unwrap()
            .passed
    );
    server.abort();
    f.close().await;
}
