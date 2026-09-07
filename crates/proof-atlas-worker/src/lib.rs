//! Finalized-chain scheduling only; model text cannot advance rounds or publish.
//! Apply shared DB migrations as the owner, never through the worker connection.
//! The external agent adapter owns process-tree cleanup.
#![forbid(unsafe_code)]

mod controller;
mod store;
mod types;

pub use controller::*;
pub use store::*;
pub use types::*;

#[derive(Debug, thiserror::Error)]
pub enum AtlasError {
    #[error("invalid Atlas configuration or runtime binding")]
    Invalid,
    #[error("Atlas runtime budget exhausted or invocation already consumed")]
    Exhausted,
    #[error("Atlas runtime interrupted; retain recovery state")]
    Interrupted,
    #[error("Atlas adapter unavailable")]
    Unavailable,
    #[error("Atlas scheduling persistence unavailable")]
    Database,
    #[error(transparent)]
    Round(#[from] proof_rounds::RoundError),
}

impl From<sqlx::Error> for AtlasError {
    fn from(_: sqlx::Error) -> Self {
        Self::Database
    }
}
impl From<proof_autonomy::ContractError> for AtlasError {
    fn from(_: proof_autonomy::ContractError) -> Self {
        Self::Invalid
    }
}
