#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../proof-autonomy-pg/tests/common/mod.rs"]
mod common;

use async_trait::async_trait;
use common::{consent, Fixture};
use proof_autonomy::{commitment, DeletionResult, MachineQuote, ProvisionResult};
use proof_autonomy_pg::{MinerAccount, Resource};
use proof_broker::{Broker, BrokerError, MinerProvider, ProviderError};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use uuid::Uuid;

struct LocalProvider {
    account: Uuid,
    credential: Uuid,
    changed: bool,
    ambiguous: bool,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl MinerProvider for LocalProvider {
    async fn preflight(
        &self,
        account: &MinerAccount,
        quote: &MachineQuote,
    ) -> Result<String, ProviderError> {
        assert_eq!(account.id, self.account);
        assert_eq!(account.credential_ref, self.credential);
        if self.changed {
            Ok("f".repeat(64))
        } else {
            Ok(commitment(quote).unwrap())
        }
    }
    async fn rent(&self, account: &MinerAccount, _: &MachineQuote, _: Uuid) -> ProvisionResult {
        assert_eq!(account.id, self.account);
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.ambiguous {
            ProvisionResult::Uncertain
        } else {
            ProvisionResult::Confirmed {
                resource_id: "local-pod".into(),
            }
        }
    }
    async fn reconcile(
        &self,
        account: &MinerAccount,
        _: &MachineQuote,
        _: Uuid,
    ) -> ProvisionResult {
        assert_eq!(account.id, self.account);
        ProvisionResult::Confirmed {
            resource_id: "local-pod".into(),
        }
    }
    async fn delete(
        &self,
        account: &MinerAccount,
        resource: &Resource,
    ) -> Result<(), ProviderError> {
        assert_eq!(account.id, self.account);
        assert_eq!(resource.resource_id, "local-pod");
        assert!(resource.deletion_id.is_some());
        Ok(())
    }
    async fn deletion_status(&self, _: &MinerAccount, _: &Resource) -> DeletionResult {
        DeletionResult::Pending
    }
}

#[tokio::test]
async fn exact_consent_drives_single_rent_and_cleanup_stays_pending_on_ack_only() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (e, lease, quote) = f.quoted().await;
    let e = f
        .store
        .consent(e.id, e.revision, &consent(&quote))
        .await
        .unwrap();
    let intent = f.store.intents(e.id, &e.miner_hotkey).await.unwrap()[0].id;
    let calls = Arc::new(AtomicUsize::new(0));
    let broker = Broker::new(
        f.store.clone(),
        LocalProvider {
            account: f.account.id,
            credential: f.account.credential_ref,
            changed: false,
            ambiguous: false,
            calls: calls.clone(),
        },
        Duration::from_secs(1),
    )
    .unwrap();
    assert!(matches!(
        broker.provision(&lease, e.revision, intent).await.unwrap(),
        ProvisionResult::Confirmed { .. }
    ));
    assert!(broker.provision(&lease, e.revision, intent).await.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        broker.cleanup(&lease, "local-pod").await.unwrap(),
        DeletionResult::Pending
    );
    assert!(f.store.finish_cleanup(&lease).await.is_err());
    f.close().await;
}

#[tokio::test]
async fn offer_change_is_rejected_before_any_dispatch_or_rent() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (e, lease, quote) = f.quoted().await;
    let e = f
        .store
        .consent(e.id, e.revision, &consent(&quote))
        .await
        .unwrap();
    let intent = f.store.intents(e.id, &e.miner_hotkey).await.unwrap()[0].id;
    let calls = Arc::new(AtomicUsize::new(0));
    let broker = Broker::new(
        f.store.clone(),
        LocalProvider {
            account: f.account.id,
            credential: f.account.credential_ref,
            changed: true,
            ambiguous: false,
            calls: calls.clone(),
        },
        Duration::from_secs(1),
    )
    .unwrap();
    assert!(matches!(
        broker.provision(&lease, e.revision, intent).await,
        Err(BrokerError::ChangedOffer)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(f.store.provision_context(&lease, intent).await.is_ok());
    f.close().await;
}

#[tokio::test]
async fn restart_reconciles_original_request_without_second_rent() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (e, old, quote) = f.quoted().await;
    let e = f
        .store
        .consent(e.id, e.revision, &consent(&quote))
        .await
        .unwrap();
    let intent = f.store.intents(e.id, &e.miner_hotkey).await.unwrap()[0].id;
    let calls = Arc::new(AtomicUsize::new(0));
    let broker = Broker::new(
        f.store.clone(),
        LocalProvider {
            account: f.account.id,
            credential: f.account.credential_ref,
            changed: false,
            ambiguous: true,
            calls: calls.clone(),
        },
        Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(
        broker.provision(&old, e.revision, intent).await.unwrap(),
        ProvisionResult::Uncertain
    );
    f.expire(&old).await;
    let lease = f.store.acquire(e.id, Uuid::new_v4(), 60).await.unwrap();
    assert!(broker.provision(&lease, e.revision, intent).await.is_err());
    assert!(matches!(
        broker.reconcile(&lease, intent).await.unwrap(),
        ProvisionResult::Confirmed { .. }
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let e = f.store.experiment(e.id, &e.miner_hotkey).await.unwrap();
    assert!(f.store.adopt_resource(&lease, e.revision).await.is_ok());
    f.close().await;
}
