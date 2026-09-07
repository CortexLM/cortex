//! The worker owns lifecycle advancement; models own neither spending consent
//! nor cleanup. Every external call is outside its database transactions.

#![forbid(unsafe_code)]

mod controller;
mod launch;
mod process;
mod runs;
mod types;

pub use controller::*;
pub use launch::*;
pub use runs::*;
pub use types::*;

#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    #[error("worker configuration or immutable state is invalid")]
    Invalid,
    #[error("worker ownership or shutdown interrupted the operation")]
    Interrupted,
    #[error("worker service is unavailable")]
    Unavailable,
    #[error("headless runtime failed: {0}")]
    Runtime(&'static str),
    #[error(transparent)]
    Store(#[from] proof_autonomy_pg::StoreError),
}

impl From<sqlx::Error> for WorkerError {
    fn from(_: sqlx::Error) -> Self {
        Self::Unavailable
    }
}
impl From<proof_research::ResearchError> for WorkerError {
    fn from(_: proof_research::ResearchError) -> Self {
        Self::Unavailable
    }
}
impl From<proof_broker::BrokerError> for WorkerError {
    fn from(_: proof_broker::BrokerError) -> Self {
        Self::Unavailable
    }
}
impl From<proof_autonomy::ContractError> for WorkerError {
    fn from(_: proof_autonomy::ContractError) -> Self {
        Self::Invalid
    }
}
impl From<std::io::Error> for WorkerError {
    fn from(_: std::io::Error) -> Self {
        Self::Unavailable
    }
}
impl From<proof_runtime::RuntimeError> for WorkerError {
    fn from(_: proof_runtime::RuntimeError) -> Self {
        Self::Unavailable
    }
}
