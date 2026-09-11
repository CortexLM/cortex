//! Data-plane modules extracted from the `gateway` crate so both stay under
//! the workspace LOC cap. `gateway` re-exports everything here; external
//! callers should keep importing `gateway::*`.
//!
//! - [`admin_auth`]: Bearer gate for `/v1/admin/*`.
//! - [`admin_attest`]: master-only owner credit for non-TEE runtimes.
//! - [`weights_store`]: raw-weight leaf row + in-memory store + ingress errors.
//! - [`proxy_detach`]: Proof-only disconnect-survive hop + path normalize.

#![forbid(unsafe_code)]

pub mod admin_attest;
pub mod admin_auth;
pub mod proxy_detach;
pub mod weights_store;
