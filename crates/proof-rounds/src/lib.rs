//! Version-2 research rounds. Frozen inputs and signed leaves are immutable.
//! Models never supply chain state, admitted credit, signer keys or destinations.

#![forbid(unsafe_code)]

mod atlas;
mod publication;
mod store;
mod types;

pub use atlas::*;
pub use publication::*;
pub use store::*;
pub use types::*;

#[derive(Debug, thiserror::Error)]
pub enum RoundError {
    #[error("round inputs or evidence are invalid")]
    Invalid,
    #[error("round ownership expired or conflicts with existing work")]
    Fenced,
    #[error("round persistence is unavailable or corrupt")]
    Database,
    #[error("round publication is unconfirmed")]
    Publication,
}

impl From<sqlx::Error> for RoundError {
    fn from(_: sqlx::Error) -> Self {
        Self::Database
    }
}
impl From<proof_autonomy::ContractError> for RoundError {
    fn from(_: proof_autonomy::ContractError) -> Self {
        Self::Invalid
    }
}
impl From<proof_research::ResearchError> for RoundError {
    fn from(_: proof_research::ResearchError) -> Self {
        Self::Invalid
    }
}
