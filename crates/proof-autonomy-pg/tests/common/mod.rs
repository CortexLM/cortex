#![allow(clippy::expect_used, clippy::unwrap_used, dead_code)]

use db::TestPool;
use proof_autonomy::{
    commitment, MachineQuote, SignedAction, SignedConsent, ACTION_DOMAIN, CONSENT_DOMAIN,
};
use proof_autonomy_pg::{ControllerLease, CreateExperiment, Experiment, MinerAccount, PgStore};
use serde::Serialize;
use uuid::Uuid;

pub const SEED: [u8; 32] = [7; 32];

pub struct Fixture {
    pub database: TestPool,
    pub store: PgStore,
    pub account: MinerAccount,
}

impl Fixture {
    pub async fn new() -> Option<Self> {
        if std::env::var_os("DATABASE_URL").is_none() {
            eprintln!("Postgres integration test skipped: DATABASE_URL is unset");
            return None;
        }
        let database = db::test_pool().await.expect("isolated schema");
        let store = PgStore::new(database.app_pool().await.expect("app role"));
        let account = MinerAccount {
            id: Uuid::new_v4(),
            miner_hotkey: miner(&SEED),
            credential_ref: Uuid::new_v4(),
        };
        store.register_account(&account).await.expect("account");
        Some(Self {
            database,
            store,
            account,
        })
    }

    pub async fn now(&self) -> u64 {
        let seconds: i64 =
            sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp()))::bigint")
                .fetch_one(self.database.pool())
                .await
                .expect("DB time");
        u64::try_from(seconds).unwrap()
    }

    pub fn request(&self) -> CreateExperiment {
        CreateExperiment {
            id: Uuid::new_v4(),
            account_id: self.account.id,
            recipe_digest: "a".repeat(64),
        }
    }

    pub async fn create(&self) -> Experiment {
        let request = self.request();
        self.store
            .create_experiment(
                &request,
                &signed(&request, "/v2/experiments", self.now().await, &SEED),
            )
            .await
            .expect("create")
    }

    pub async fn quoted(&self) -> (Experiment, ControllerLease, MachineQuote) {
        let experiment = self.create().await;
        let lease = self
            .store
            .acquire(experiment.id, Uuid::new_v4(), 60)
            .await
            .expect("lease");
        let now = self.now().await;
        let quote = MachineQuote {
            schema_version: 1,
            id: Uuid::new_v4(),
            experiment_id: experiment.id,
            miner_hotkey: experiment.miner_hotkey.clone(),
            account_id: experiment.account_id,
            recipe_digest: experiment.recipe_digest.clone(),
            offer_id: "local-fake-offer".into(),
            gpu_type: "test-gpu".into(),
            gpu_count: 1,
            gpu_memory_mib: 100,
            ram_mib: 100,
            disk_gib: 10,
            image: "invalid.example/test-only".into(),
            image_digest: format!("sha256:{}", "b".repeat(64)),
            hourly_total_microusd: 1_000_000,
            maximum_total_microusd: 1_000_000,
            lifetime_seconds: 3_600,
            issued_at: now,
            expires_at: now + 180,
            provider_fingerprint: "c".repeat(64),
        };
        let experiment = self
            .store
            .publish_quote(&lease, 0, &quote)
            .await
            .expect("quote");
        (experiment, lease, quote)
    }

    pub async fn expire(&self, lease: &ControllerLease) {
        sqlx::query(
            "UPDATE proof_controller_lease SET expires_at = clock_timestamp() - interval '1 second' \
             WHERE experiment_id = $1",
        )
        .bind(lease.experiment_id)
        .execute(self.database.pool())
        .await
        .expect("simulate expired controller");
    }

    pub async fn close(self) {
        self.database.drop_schema().await.expect("drop test schema");
    }
}

pub fn miner(seed: &[u8; 32]) -> String {
    hex::encode(challenge_common::public_key_from_secret(seed).unwrap())
}

pub fn signed<T: Serialize>(body: &T, path: &str, now: u64, seed: &[u8; 32]) -> SignedAction {
    let mut action = SignedAction {
        miner_hotkey: miner(seed),
        nonce: Uuid::new_v4(),
        expires_at: now + 120,
        method: "POST".into(),
        path: path.into(),
        body_digest: commitment(body).unwrap(),
        signature: String::new(),
    };
    sign_action(&mut action, seed);
    action
}

pub fn sign_action(action: &mut SignedAction, seed: &[u8; 32]) {
    let payload = (
        &action.miner_hotkey,
        action.nonce,
        action.expires_at,
        &action.method,
        &action.path,
        &action.body_digest,
    );
    action.signature = hex::encode(
        crypto::sign_raw(
            seed,
            ACTION_DOMAIN,
            commitment(&payload).unwrap().as_bytes(),
        )
        .unwrap(),
    );
}

pub fn consent(quote: &MachineQuote) -> SignedConsent {
    let digest = commitment(quote).unwrap();
    SignedConsent {
        signature: hex::encode(crypto::sign_raw(&SEED, CONSENT_DOMAIN, digest.as_bytes()).unwrap()),
        quote_digest: digest,
    }
}
