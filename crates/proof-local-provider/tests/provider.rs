#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "../../proof-autonomy-pg/tests/common/mod.rs"]
mod common;

use std::path::Path;

use common::{consent, Fixture};
use proof_autonomy::{commitment, DeletionResult, ProvisionResult};
use proof_autonomy_pg::{ControllerLease, Experiment, MinerAccount, Resource};
use proof_broker::{Broker, MinerProvider};
use proof_local_provider::{
    LocalProvider, LocalProviderConfig, LOCAL_IMAGE_NAME, PLACEHOLDER_HOURLY_MICROUSD,
};
use proof_worker::QuoteSource;
use uuid::Uuid;

const IMAGE: &str = "sha256:0104307df448338d8475c7cf8152e5e0655e211fd1c04b2bdc94e6758a7e7293";
const SOCKET: &str = "/var/run/docker.sock";

fn config(slots: u32) -> LocalProviderConfig {
    LocalProviderConfig {
        docker_socket: Path::new(SOCKET).into(),
        image_id: IMAGE.into(),
        slots,
        lifetime_seconds: 3_600,
        quote_seconds: 300,
        ram_mib: 256,
    }
}

struct Test {
    f: Fixture,
    provider: LocalProvider,
}

impl Test {
    async fn new(slots: u32) -> Option<Self> {
        let f = Fixture::new().await?;
        if !Path::new(SOCKET).exists() {
            eprintln!("Docker integration test skipped: {SOCKET} is absent");
            f.close().await;
            return None;
        }
        let pool = f.database.app_pool().await.unwrap();
        let provider = LocalProvider::connect(pool, config(slots)).await.unwrap();
        provider.ensure_slots().await.unwrap();
        Some(Self { f, provider })
    }

    /// Drive one experiment through quote → consent → dispatched intent.
    async fn dispatched(&self) -> (Experiment, ControllerLease, Uuid) {
        let experiment = self.f.create().await;
        let lease = self
            .f
            .store
            .acquire(experiment.id, Uuid::new_v4(), 60)
            .await
            .unwrap();
        let quote = self.provider.quote(&experiment).await.unwrap().unwrap();
        assert_eq!(quote.image, LOCAL_IMAGE_NAME);
        assert_eq!(quote.image_digest, IMAGE);
        assert_eq!(quote.hourly_total_microusd, PLACEHOLDER_HOURLY_MICROUSD);
        let e = self
            .f
            .store
            .publish_quote(&lease, experiment.revision, &quote)
            .await
            .unwrap();
        let e = self
            .f
            .store
            .consent(e.id, e.revision, &consent(&quote))
            .await
            .unwrap();
        let intent = self.f.store.intents(e.id, &e.miner_hotkey).await.unwrap()[0].id;
        (e, lease, intent)
    }

    async fn provisioned(&self) -> (Experiment, ControllerLease, ProvisionResult) {
        let (e, lease, intent) = self.dispatched().await;
        let broker = Broker::new(
            self.f.store.clone(),
            self.provider.clone(),
            std::time::Duration::from_secs(10),
        )
        .unwrap();
        let result = broker.provision(&lease, e.revision, intent).await.unwrap();
        (e, lease, result)
    }

    async fn resource(&self, e: &Experiment) -> Resource {
        let resources = self.f.store.resources(e.id, &e.miner_hotkey).await.unwrap();
        resources.into_iter().next().unwrap()
    }

    async fn slots_claimed(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM proof_local_slot WHERE intent_id IS NOT NULL")
            .fetch_one(self.f.database.pool())
            .await
            .unwrap()
    }
}

#[test]
fn configuration_bounds_are_enforced() {
    assert!(config(1).validate().is_ok());
    assert!(config(0).validate().is_err());
    assert!(config(65).validate().is_err());
    let mut c = config(1);
    c.image_id = "cortex-atlas-kernel:local-test".into();
    assert!(c.validate().is_err());
    let mut c = config(1);
    c.docker_socket = "var/run/docker.sock".into();
    assert!(c.validate().is_err());
    let mut c = config(1);
    c.lifetime_seconds = 1;
    assert!(c.validate().is_err());
}

#[tokio::test]
async fn quotes_are_pinned_and_preflight_reobserves_them() {
    let Some(t) = Test::new(2).await else {
        return;
    };
    let e = t.f.create().await;
    let quote = t.provider.quote(&e).await.unwrap().unwrap();
    assert_eq!(quote.validate(t.f.now().await), Ok(()));
    assert_eq!(quote.maximum_total_microusd, 1);
    assert_eq!(
        t.provider.preflight(&t.f.account, &quote).await.unwrap(),
        commitment(&quote).unwrap()
    );
    let mut foreign = quote.clone();
    foreign.image_digest = format!("sha256:{}", "b".repeat(64));
    assert!(t.provider.preflight(&t.f.account, &foreign).await.is_err());
    let mut priced = quote.clone();
    priced.hourly_total_microusd = 1_000_000;
    assert!(t.provider.preflight(&t.f.account, &priced).await.is_err());
    // A quote not issued here can never claim a slot.
    assert_eq!(
        t.provider.rent(&t.f.account, &priced, Uuid::new_v4()).await,
        ProvisionResult::NotCreated
    );
    assert_eq!(t.slots_claimed().await, 0);
    t.f.close().await;
}

#[tokio::test]
async fn slots_are_exhausted_and_each_intent_claims_at_most_once() {
    let Some(t) = Test::new(2).await else {
        return;
    };
    let (e1, _, r1) = t.provisioned().await;
    let (_, _, r2) = t.provisioned().await;
    let (_, _, r3) = t.provisioned().await;
    assert!(matches!(r1, ProvisionResult::Confirmed { .. }), "{r1:?}");
    assert!(matches!(r2, ProvisionResult::Confirmed { .. }));
    assert_eq!(r3, ProvisionResult::NotCreated);
    assert_eq!(t.slots_claimed().await, 2);
    let first = t.resource(&e1).await;
    assert!(first.resource_id.starts_with("local-"));
    // Repeating the exact intent is idempotent; no second slot is consumed.
    let quote = t.provider.quote(&e1).await.unwrap().unwrap();
    assert_eq!(
        t.provider.rent(&t.f.account, &quote, first.intent_id).await,
        ProvisionResult::Confirmed {
            resource_id: first.resource_id.clone()
        }
    );
    assert_eq!(t.slots_claimed().await, 2);
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM proof_local_event WHERE intent_id = $1 AND kind = 'claimed'",
    )
    .bind(first.intent_id)
    .fetch_one(t.f.database.pool())
    .await
    .unwrap();
    assert_eq!(events, 1);
    t.f.close().await;
}

#[tokio::test]
async fn reconcile_reads_by_intent_and_seals_unknown_requests() {
    let Some(t) = Test::new(1).await else {
        return;
    };
    let (e, _, r) = t.provisioned().await;
    let resource = t.resource(&e).await;
    let quote = t.provider.quote(&e).await.unwrap().unwrap();
    assert_eq!(
        t.provider
            .reconcile(&t.f.account, &quote, resource.intent_id)
            .await,
        r
    );
    // An intent that never reached the provider is refused forever, so a
    // later original request cannot claim a slot.
    let (_, _, unknown) = t.dispatched().await;
    assert_eq!(
        t.provider.reconcile(&t.f.account, &quote, unknown).await,
        ProvisionResult::NotCreated
    );
    assert_eq!(
        t.provider.rent(&t.f.account, &quote, unknown).await,
        ProvisionResult::NotCreated
    );
    assert_eq!(t.slots_claimed().await, 1);
    t.f.close().await;
}

#[tokio::test]
async fn delete_releases_the_slot_and_confirms_absent_containers() {
    let Some(t) = Test::new(1).await else {
        return;
    };
    let (e, lease, _) = t.provisioned().await;
    let resource = t.resource(&e).await;
    let foreign = MinerAccount {
        id: Uuid::new_v4(),
        miner_hotkey: t.f.account.miner_hotkey.clone(),
        credential_ref: Uuid::new_v4(),
    };
    assert_eq!(
        t.provider.delete(&foreign, &resource).await,
        Err(proof_broker::ProviderError::Unauthorized)
    );
    assert_eq!(
        t.provider.deletion_status(&foreign, &resource).await,
        DeletionResult::Unauthorized
    );
    assert_eq!(
        t.provider.deletion_status(&t.f.account, &resource).await,
        DeletionResult::Pending
    );
    let broker = Broker::new(
        t.f.store.clone(),
        t.provider.clone(),
        std::time::Duration::from_secs(10),
    )
    .unwrap();
    assert_eq!(
        broker.cleanup(&lease, &resource.resource_id).await.unwrap(),
        DeletionResult::Confirmed
    );
    assert_eq!(t.slots_claimed().await, 0);
    assert_eq!(t.resource(&e).await.status, "deleted");
    // Repeating the deletion stays confirmed and appends nothing new.
    assert_eq!(t.provider.delete(&t.f.account, &resource).await, Ok(()));
    assert_eq!(
        t.provider.deletion_status(&t.f.account, &resource).await,
        DeletionResult::Confirmed
    );
    let released: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM proof_local_event WHERE intent_id = $1 AND kind = 'released'",
    )
    .bind(resource.intent_id)
    .fetch_one(t.f.database.pool())
    .await
    .unwrap();
    assert_eq!(released, 1);
    // The freed slot is claimable again.
    let (_, _, again) = t.provisioned().await;
    assert!(matches!(again, ProvisionResult::Confirmed { .. }));
    t.f.close().await;
}
