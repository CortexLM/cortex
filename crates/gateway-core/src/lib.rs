//! Data-plane modules extracted from the `gateway` crate so both stay under
//! the workspace LOC cap. `gateway` re-exports everything here; external
//! callers should keep importing `gateway::*`.
//!
//! - [`admin_auth`]: Bearer gate for `/v1/admin/*`.
//! - [`admin_attest`]: master-only owner credit for non-TEE runtimes.
//! - [`weights_store`]: raw-weight leaf row + in-memory store + ingress errors.
//! - [`proxy_detach`]: Proof-only disconnect-survive hop + path normalize.
//! - [`topic_routes`]: the `/challenge/{topic_id}/…` rule — a topic id the
//!   registry does not know is forwarded to the Proof challenge, which
//!   resolves it against `proof_topic_api`.

#![forbid(unsafe_code)]

pub mod admin_attest;
pub mod admin_auth;
pub mod proxy_detach;
pub mod topic_routes;
pub mod weights_store;
