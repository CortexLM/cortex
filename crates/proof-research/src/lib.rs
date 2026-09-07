//! Controller-owned evidence. A model verdict cannot replace observations,
//! retained artifacts, signed topic gates, or confirmed publication.

#![forbid(unsafe_code)]

mod corpus;
mod flops;
mod publication;
mod science;
mod store;

pub use corpus::*;
pub use flops::{flop_total, ComputeOp, ComputeTrace};
pub use publication::*;
pub use science::*;
pub use store::*;

#[derive(Debug, thiserror::Error)]
pub enum ResearchError {
    #[error("scientific evidence is incomplete or outside the committed recipe")]
    Evidence,
    #[error("stored scientific record is invalid")]
    Corrupt,
    #[error("publication remains unconfirmed")]
    Publication,
    #[error(transparent)]
    Store(#[from] proof_autonomy_pg::StoreError),
    #[error("scientific persistence failed")]
    Database,
}

impl From<sqlx::Error> for ResearchError {
    fn from(_: sqlx::Error) -> Self {
        Self::Database
    }
}

impl From<proof_autonomy::ContractError> for ResearchError {
    fn from(_: proof_autonomy::ContractError) -> Self {
        Self::Evidence
    }
}
