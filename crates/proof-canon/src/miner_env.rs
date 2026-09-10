//! Miner BYOK: the environment a miner brings to their own paid run.
//!
//! A topic that needs a third-party credential the **miner** pays for (an
//! inference key, an API token) says so in its signed document — it never
//! ships the operator's. The miner posts the value on `POST /v1/submissions`
//! as `env: { "<NAME>": "<value>" }`; the host keeps only the names the
//! signed topic declared and hands those to the guest that runs the miner's
//! code. Everything else is refused by name.
//!
//! Two knobs, both `constraints.params` entries of the signed topic:
//!
//! | Param | Meaning |
//! |-------|---------|
//! | [`PARAM_MINER_ENV_ALLOWLIST`] | comma-separated names a miner **may** send |
//! | [`PARAM_MINER_BYOK`] | one name a miner **must** send (also allowed) |
//!
//! A topic that declares neither takes no miner env at all: a body that
//! carries one is refused, so a miner can never smuggle a variable into a
//! guest a topic did not ask for. The names are the topic's; this crate
//! knows the shape only and never a provider, a vendor, or a key.
//!
//! [`MinerEnv`] is the carrier. It is a plain map on the wire, but its
//! [`Debug`] prints names only — a value in this type never reaches a log,
//! a journal line, or a panic message by accident.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::Constraints;

#[cfg(test)]
#[path = "miner_env_tests.rs"]
mod tests;

/// `constraints.params` key naming the **one** environment variable a miner
/// must supply for every paid run of this topic (bring-your-own-key).
///
/// The value is the variable's name (`is_env_name`), never a key. A topic
/// that sets it refuses a submission that omits it — before any spend.
pub const PARAM_MINER_BYOK: &str = "miner_byok";

/// `constraints.params` key listing every environment variable a miner **may**
/// supply, comma-separated. Optional: a name here is accepted, never demanded.
/// [`PARAM_MINER_BYOK`] is always allowed on top of this list.
pub const PARAM_MINER_ENV_ALLOWLIST: &str = "miner_env_allowlist";

/// `constraints.params` key (`"true"` / `"false"`) that also forwards the
/// miner env into the **sister** guest running the miner's own entrypoint.
/// Absent / `"false"` keeps it in the runner guest only.
pub const PARAM_INJECT_MINER_ENV_SISTER: &str = "inject_miner_env_sister";

/// Most variables one submission may carry.
pub const MAX_MINER_ENV_VARS: usize = 8;

/// Longest value one variable may carry.
pub const MAX_MINER_ENV_VALUE_LEN: usize = 4_096;

/// Longest variable name.
pub const MAX_ENV_NAME_LEN: usize = 64;

/// Prefix the guest's own adaptor contract owns. A miner variable may never
/// start with it: the contract's values are host facts, not miner input.
pub const RESERVED_ENV_PREFIX: &str = "PROOF_";

/// Names the guest sets for every adaptor. A miner may not shadow them.
pub const RESERVED_ENV_NAMES: &[&str] = &["PATH", "HOME", "LANG", "XDG_RUNTIME_DIR"];

/// Whether `s` is a POSIX-shaped environment variable name a miner may bring:
/// `[A-Z][A-Z0-9_]{0,63}`, not `PROOF_…`, not one the guest already sets.
///
/// Upper case only — the guest exports these verbatim beside its own
/// contract, and a lower-case twin of a contract name is exactly the
/// confusion this shape rules out.
#[must_use]
pub fn is_env_name(s: &str) -> bool {
    let ok_shape = !s.is_empty()
        && s.len() <= MAX_ENV_NAME_LEN
        && s.starts_with(|c: char| c.is_ascii_uppercase())
        && s.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
    ok_shape && !s.starts_with(RESERVED_ENV_PREFIX) && !RESERVED_ENV_NAMES.contains(&s)
}

/// Why a submission's `env` is not acceptable for a topic.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MinerEnvError {
    /// The name is not a variable name a miner may bring.
    #[error("env name {name:?} is not a miner environment variable ([A-Z][A-Z0-9_]{{0,63}}, never {RESERVED_ENV_PREFIX}… and never one the guest already sets)")]
    BadName {
        /// The offending name, as posted.
        name: String,
    },
    /// The signed topic does not declare this name.
    #[error("env name {name:?} is not declared by this topic; it accepts {allowed}")]
    Undeclared {
        /// The offending name.
        name: String,
        /// What the topic does declare (`(none)` when it declares nothing).
        allowed: String,
    },
    /// The topic requires this name and the body did not carry it.
    #[error("env.{name} is required by this topic ({PARAM_MINER_BYOK}); send it in the submit body — the operator key is never used for a miner run")]
    Missing {
        /// The name the topic demands.
        name: String,
    },
    /// The value was empty or blank.
    #[error("env.{name} is empty")]
    Empty {
        /// The name whose value was blank.
        name: String,
    },
    /// The value is too long or not a single printable line.
    #[error("env.{name} must be a single printable line of at most {MAX_MINER_ENV_VALUE_LEN} characters")]
    BadValue {
        /// The name whose value is malformed.
        name: String,
    },
    /// Too many variables in one body.
    #[error("env carries {got} variables (at most {MAX_MINER_ENV_VARS})")]
    TooMany {
        /// How many were posted.
        got: usize,
    },
}

/// Environment a miner supplies for their own run: `NAME → value`.
///
/// Serialises as the plain JSON object it is on the wire. [`Debug`] prints
/// the **names** and never a value, so a value cannot reach a log line by
/// being part of a bigger structure someone formatted.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MinerEnv(BTreeMap<String, String>);

impl fmt::Debug for MinerEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MinerEnv")
            .field("names", &self.names())
            .field("values", &"[REDACTED]")
            .finish()
    }
}

impl MinerEnv {
    /// An empty environment: what every topic that declares no BYOK gets.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether nothing is carried.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How many variables are carried.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// The names carried, sorted. Safe to log; the values are not.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        self.0.keys().map(String::as_str).collect()
    }

    /// Every `(name, value)` pair, sorted by name.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// The values carried, as bytes — the redaction material a guest blanks
    /// out of anything it ships back.
    #[must_use]
    pub fn secret_values(&self) -> Vec<Vec<u8>> {
        self.0.values().map(|v| v.as_bytes().to_vec()).collect()
    }

    /// Add one variable (replacing an earlier value of the same name).
    pub fn insert(&mut self, name: &str, value: &str) {
        self.0.insert(name.to_owned(), value.to_owned());
    }

    /// The environment this topic accepts from this body, or the first reason
    /// it does not.
    ///
    /// Names are trimmed, then checked in this order: shape, declaration,
    /// value. Anything the signed topic does not declare is
    /// [`MinerEnvError::Undeclared`] — never dropped silently — and a name
    /// the topic demands but the body omits is [`MinerEnvError::Missing`].
    /// The result carries the declared names and nothing else, so a topic
    /// that widens or narrows its allowlist changes what reaches the guest
    /// without any host state moving.
    ///
    /// # Errors
    ///
    /// [`MinerEnvError`].
    pub fn accept(&self, constraints: &Constraints) -> Result<Self, MinerEnvError> {
        if self.0.len() > MAX_MINER_ENV_VARS {
            return Err(MinerEnvError::TooMany { got: self.0.len() });
        }
        let allowed = constraints.miner_env_allowlist();
        let mut kept = BTreeMap::new();
        for (raw_name, value) in &self.0 {
            let name = raw_name.trim();
            if !is_env_name(name) {
                return Err(MinerEnvError::BadName {
                    name: raw_name.clone(),
                });
            }
            if !allowed.iter().any(|a| a == name) {
                return Err(MinerEnvError::Undeclared {
                    name: name.to_owned(),
                    allowed: name_list(&allowed),
                });
            }
            let value = value.trim();
            if value.is_empty() {
                return Err(MinerEnvError::Empty {
                    name: name.to_owned(),
                });
            }
            if value.len() > MAX_MINER_ENV_VALUE_LEN || !is_single_printable_line(value) {
                return Err(MinerEnvError::BadValue {
                    name: name.to_owned(),
                });
            }
            kept.insert(name.to_owned(), value.to_owned());
        }
        for required in constraints.miner_env_required() {
            if !kept.contains_key(&required) {
                return Err(MinerEnvError::Missing { name: required });
            }
        }
        Ok(Self(kept))
    }
}

/// `A, B` — or `(none)` for an empty list. Names only; never a value.
fn name_list(names: &[String]) -> String {
    if names.is_empty() {
        "(none)".to_owned()
    } else {
        names.join(", ")
    }
}

fn is_single_printable_line(s: &str) -> bool {
    !s.chars().any(char::is_control)
}

/// Names in a comma-separated param value, trimmed, in order, deduplicated.
fn split_names(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in raw.split(',') {
        let name = part.trim();
        if !name.is_empty() && !out.iter().any(|n| n == name) {
            out.push(name.to_owned());
        }
    }
    out
}

impl Constraints {
    /// Every environment variable name this signed topic lets a miner supply:
    /// [`PARAM_MINER_ENV_ALLOWLIST`] plus [`PARAM_MINER_BYOK`]. Empty for a
    /// topic that declares neither — which takes no miner env at all.
    ///
    /// A malformed entry is dropped here rather than widening the list; a
    /// document that went through [`Constraints::validate_shape`] has none.
    #[must_use]
    pub fn miner_env_allowlist(&self) -> Vec<String> {
        let mut names = self
            .params
            .get(PARAM_MINER_ENV_ALLOWLIST)
            .map(|raw| split_names(raw))
            .unwrap_or_default();
        for required in self.miner_env_required() {
            if !names.contains(&required) {
                names.push(required);
            }
        }
        names.retain(|n| is_env_name(n));
        names
    }

    /// The names this topic **demands** on every submission
    /// ([`PARAM_MINER_BYOK`]). A submission missing one is refused before any
    /// row, rent, or paid inference.
    #[must_use]
    pub fn miner_env_required(&self) -> Vec<String> {
        self.params
            .get(PARAM_MINER_BYOK)
            .map(|raw| split_names(raw))
            .unwrap_or_default()
            .into_iter()
            .filter(|n| is_env_name(n))
            .collect()
    }

    /// Whether the miner env is also forwarded into the sister guest
    /// ([`PARAM_INJECT_MINER_ENV_SISTER`]). Absent / malformed reads as
    /// `false`: the flag can only ever widen where the miner's own key goes,
    /// so it must be said explicitly.
    #[must_use]
    pub fn inject_miner_env_sister(&self) -> bool {
        self.params
            .get(PARAM_INJECT_MINER_ENV_SISTER)
            .and_then(|v| crate::parse_bool_param(v))
            .unwrap_or(false)
    }

    /// Shape of the BYOK knobs, checked at publish so a topic can never open
    /// with an allowlist nothing can honour.
    pub(crate) fn validate_miner_env(&self) -> Result<(), crate::ShapeError> {
        for key in [PARAM_MINER_BYOK, PARAM_MINER_ENV_ALLOWLIST] {
            let Some(raw) = self.params.get(key) else {
                continue;
            };
            let names = split_names(raw);
            if names.is_empty() || !names.iter().all(|n| is_env_name(n)) {
                return Err(crate::ShapeError {
                    field: format!("constraints.params.{key}"),
                    why: "comma-separated [A-Z][A-Z0-9_]{0,63} names, never PROOF_… and never one the guest already sets",
                });
            }
        }
        if self
            .params
            .get(PARAM_INJECT_MINER_ENV_SISTER)
            .is_some_and(|v| crate::parse_bool_param(v).is_none())
        {
            return Err(crate::ShapeError {
                field: format!("constraints.params.{PARAM_INJECT_MINER_ENV_SISTER}"),
                why: "\"true\" or \"false\"",
            });
        }
        Ok(())
    }
}
