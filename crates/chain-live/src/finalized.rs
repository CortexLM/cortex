use chain::{ChainClient, ChainError, Metagraph};

use crate::{storage, LiveChainClient, PALLET_SUBTENSOR};

/// Read-only, hash-pinned chain inputs for a closed research window.
#[derive(Debug, Clone)]
pub struct FinalizedSnapshot {
    pub block: u64,
    pub hash: [u8; 32],
    pub chain_epoch: u64,
    pub timestamp_ms: u64,
    pub metagraph: Metagraph,
}

impl LiveChainClient {
    /// # Errors
    /// Missing finality, malformed response or transport failure.
    pub fn finalized_height(&self) -> Result<u64, ChainError> {
        self.rpc.finalized_height()
    }

    /// Refuse an optimistic block. Epoch and metagraph come from the same
    /// boundary hash, not the current mutable schedule.
    ///
    /// # Errors
    /// Unfinalized boundary, missing epoch, invalid metagraph or RPC failure.
    pub fn finalized_snapshot(&self, block: u64) -> Result<FinalizedSnapshot, ChainError> {
        if block > self.finalized_height()? {
            return Err(ChainError::Other(
                "research boundary is not finalized".into(),
            ));
        }
        let hash = self.block_hash(block)?;
        // Zero is the legacy metagraph client's "use tip" sentinel.
        if hash == [0; 32] {
            return Err(ChainError::Other(
                "research boundary hash is missing".into(),
            ));
        }
        let key = storage::storage_map_key_u16(PALLET_SUBTENSOR, "SubnetEpochIndex", self.netuid);
        let bytes = self
            .rpc
            .state_get_storage_at(&key, &hash)?
            .ok_or_else(|| ChainError::Other("research boundary has no epoch".into()))?;
        let chain_epoch = storage::decode_u64(&bytes)?;
        if chain_epoch == 0 {
            return Err(ChainError::Other("research boundary has no epoch".into()));
        }
        let timestamp = self
            .rpc
            .state_get_storage_at(&storage::storage_key("Timestamp", "Now"), &hash)?
            .ok_or_else(|| ChainError::Other("research boundary has no timestamp".into()))?;
        Ok(FinalizedSnapshot {
            block,
            hash,
            chain_epoch,
            timestamp_ms: storage::decode_u64(&timestamp)?,
            metagraph: self.metagraph_at(&hash)?,
        })
    }
}
