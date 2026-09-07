use std::sync::Arc;

use async_trait::async_trait;
use chain_live::LiveChainClient;
use serde::{Deserialize, Serialize};

use crate::Failure;

/// Controller-observed finalized provenance, never an agent argument.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochObservation {
    pub chain_epoch: u64,
    pub block: u64,
    pub hash: [u8; 32],
}

#[async_trait]
pub trait TrustedChain: Send + Sync {
    async fn finalized_epoch(&self) -> Result<EpochObservation, Failure>;
}

/// Read-only adapter; constructing it does not load a wallet or signing key.
pub struct LiveEpoch(pub Arc<LiveChainClient>);

#[async_trait]
impl TrustedChain for LiveEpoch {
    async fn finalized_epoch(&self) -> Result<EpochObservation, Failure> {
        let client = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let block = client.finalized_height().map_err(|_| Failure::Chain)?;
            let snapshot = client
                .finalized_snapshot(block)
                .map_err(|_| Failure::Chain)?;
            Ok(EpochObservation {
                chain_epoch: snapshot.chain_epoch,
                block,
                hash: snapshot.hash,
            })
        })
        .await
        .map_err(|_| Failure::Chain)?
    }
}
