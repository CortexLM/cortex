//! KVM-host plumbing for the Proof Firecracker backend.
//!
//! Three things the host needs before a VM exists, split out of
//! `proof-fc-host` so that crate stays under the workspace LOC cap:
//!
//! - [`config`] — the operator's host config: paths, pins, sizes, and the
//!   egress allowlist. Paths and pins only; no secret ever lives here.
//! - [`shell`] — the [`Shell`] trait every host command is rendered through,
//!   so the tests assert the exact argv without spawning anything, plus the
//!   recording and failing shells those tests use.
//! - [`net`] — one [`NetPlan`] per VM: a TAP on its own /30, NAT through the
//!   uplink, and an nftables table that lets the guest reach **only** the
//!   operator's egress allowlist. The allocator picks an index this host does
//!   not already have, because a jailed VM outlives the agent that booted it.

#![forbid(unsafe_code)]
#![allow(clippy::missing_errors_doc, clippy::module_name_repetitions)]

pub mod config;
pub mod net;
pub mod shell;

pub use config::{EgressAllow, HostConfig, Proto};
pub use net::NetPlan;
pub use shell::{RecordingShell, Shell, SystemShell};
