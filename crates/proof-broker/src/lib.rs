//! Trusted host orchestration. No provider credential, arbitrary URL, resource
//! name lookup, or retry-to-rent fallback is exposed to an agent.
//!
//! The legacy Lium API adapter does NOT implement this strict contract. A live
//! implementation must prove exact expiry/price enforcement and authoritative
//! request-id reconciliation before it can be selected. Tests use local fakes.

#![forbid(unsafe_code)]

use std::time::Duration;

use async_trait::async_trait;
use proof_autonomy::{commitment, DeletionResult, MachineQuote, ProvisionResult};
use proof_autonomy_pg::{ControllerLease, MinerAccount, PgStore, Resource, StoreError};
use tokio::time::timeout;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProviderError {
    #[error("provider cannot enforce the approved request")]
    Unsupported,
    #[error("provider request failed")]
    Unavailable,
    #[error("provider authorization denied")]
    Unauthorized,
}

/// Implemented only by a trusted credential broker. Resolve `credential_ref`
/// from a miner-scoped keystore; never fall back to an operator/process key.
/// No methods accept names as authority. Do not log response bodies or keys.
#[async_trait]
pub trait MinerProvider: Send + Sync {
    /// Return the commitment of the *freshly observed* exact offer, including
    /// pinned template/image, account, total price and absolute expiry. Refuse
    /// if these cannot be enforced; copying the input commitment is not enough.
    async fn preflight(
        &self,
        account: &MinerAccount,
        quote: &MachineQuote,
    ) -> Result<String, ProviderError>;

    /// At most one call per persisted intent. Enforce absolute quote expiry,
    /// lifetime and total budget at the provider. Any ambiguous transport or
    /// parser failure is `Uncertain`, never `NotCreated`.
    async fn rent(
        &self,
        account: &MinerAccount,
        quote: &MachineQuote,
        intent: Uuid,
    ) -> ProvisionResult;

    /// Read by authenticated account + provider request id, never by pod name.
    /// `NotCreated` certifies the original request cannot create a resource later.
    /// Unsupported reconciliation must return Uncertain.
    async fn reconcile(
        &self,
        account: &MinerAccount,
        quote: &MachineQuote,
        intent: Uuid,
    ) -> ProvisionResult;

    /// Delete only this stored target. Repeating the same deletion id is safe.
    async fn delete(
        &self,
        account: &MinerAccount,
        resource: &Resource,
    ) -> Result<(), ProviderError>;

    /// A DELETE acknowledgement, missing array, 401, 403 or generic 404 is not
    /// proof of billing termination. Confirmed requires authoritative identity-
    /// matched absence AND stopped billing for the target account/resource.
    async fn deletion_status(&self, account: &MinerAccount, resource: &Resource) -> DeletionResult;
}

#[derive(Debug, thiserror::Error)]
pub enum BrokerError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error("provider offer differs from approved quote")]
    ChangedOffer,
}

pub struct Broker<P> {
    store: PgStore,
    provider: P,
    timeout: Duration,
}

impl<P: MinerProvider> Broker<P> {
    /// # Errors
    /// Refuse unbounded provider calls.
    pub fn new(
        store: PgStore,
        provider: P,
        request_timeout: Duration,
    ) -> Result<Self, BrokerError> {
        if request_timeout.is_zero() || request_timeout > Duration::from_secs(30) {
            return Err(ProviderError::Unsupported.into());
        }
        Ok(Self {
            store,
            provider,
            timeout: request_timeout,
        })
    }

    /// Exact preflight → committed dispatch → one provider call → durable result.
    /// Dropped tasks and failed result persistence leave reconciliation work.
    ///
    /// # Errors
    /// Failed authorization/preflight or durable persistence. Never auto-rents again.
    pub async fn provision(
        &self,
        lease: &ControllerLease,
        revision: i64,
        intent: Uuid,
    ) -> Result<ProvisionResult, BrokerError> {
        let context = self.store.provision_context(lease, intent).await?;
        let observed = timeout(
            self.timeout,
            self.provider.preflight(&context.account, &context.quote),
        )
        .await
        .map_err(|_| ProviderError::Unavailable)??;
        if observed != commitment(&context.quote).map_err(StoreError::from)? {
            return Err(BrokerError::ChangedOffer);
        }
        let quote = self.store.begin_provision(lease, revision, intent).await?;
        if quote != context.quote {
            return Err(BrokerError::ChangedOffer);
        }
        if let Err(error) = self.store.check_dispatch(lease, intent).await {
            // This worker has not issued a request. Retain that fact even after
            // cancellation; a later worker must never retry this consumed intent.
            self.store
                .record_provision(lease, intent, &ProvisionResult::NotCreated)
                .await?;
            return Err(error.into());
        }
        let result = timeout(
            self.timeout,
            self.provider.rent(&context.account, &quote, intent),
        )
        .await
        .unwrap_or(ProvisionResult::Uncertain);
        self.store.record_provision(lease, intent, &result).await?;
        Ok(result)
    }

    /// Resume an uncertain operation by inspecting it, never by renting again.
    ///
    /// # Errors
    /// Stale owner, no uncertain operation or failed persistence.
    pub async fn reconcile(
        &self,
        lease: &ControllerLease,
        intent: Uuid,
    ) -> Result<ProvisionResult, BrokerError> {
        let (context, dispatcher) = self.store.reconciliation_context(lease, intent).await?;
        let result = timeout(
            self.timeout,
            self.provider
                .reconcile(&context.account, &context.quote, intent),
        )
        .await
        .unwrap_or(ProvisionResult::Uncertain);
        self.store
            .record_provision(&dispatcher, intent, &result)
            .await?;
        Ok(result)
    }

    /// Model-independent cleanup with separate billing-termination verification.
    ///
    /// # Errors
    /// Stale lease, wrong scope or failed persistence. Provider refusal is
    /// recorded as an unresolved result rather than reported as deletion.
    pub async fn cleanup(
        &self,
        lease: &ControllerLease,
        resource_id: &str,
    ) -> Result<DeletionResult, BrokerError> {
        let target = self.store.begin_cleanup(lease, resource_id).await?;
        let deletion_id = target.resource.deletion_id.ok_or(StoreError::Corrupt)?;
        let result = match timeout(
            self.timeout,
            self.provider.delete(&target.account, &target.resource),
        )
        .await
        {
            Ok(Ok(())) => timeout(
                self.timeout,
                self.provider
                    .deletion_status(&target.account, &target.resource),
            )
            .await
            .unwrap_or(DeletionResult::Unavailable),
            Ok(Err(ProviderError::Unauthorized)) => DeletionResult::Unauthorized,
            Ok(Err(_)) | Err(_) => DeletionResult::Unavailable,
        };
        self.store
            .record_deletion(lease, resource_id, deletion_id, &result)
            .await?;
        Ok(result)
    }
}
