//! Generic **run policy**: the well-known, shape-checked `constraints.params`
//! knobs that steer one in-guest paid job — which items of the pinned pack
//! are scored, how many, under which wall clocks, and what an item the
//! miner's harness crashed on counts as.
//!
//! Every value is topic data read from the signed document. This module
//! knows the **shape** of each knob and nothing about what a task is: the
//! operator adaptor in the guest interprets the values (a benchmark adaptor
//! maps `tasks` onto task directories, another runner onto whatever its
//! pack holds). Nothing here is a default that could score anything — a
//! topic that sets none of these gets the adaptor's contract behaviour
//! (score every item of the pack, fail closed on an unmeasured one).
//!
//! The same [`RunPolicy`] is read on both sides of the boundary: the control
//! plane refuses a malformed value **before any VM exists** (`run_paid_job`),
//! and the guest agent refuses it **before the adaptor runs** (no spend) —
//! a typo in a signed knob never runs a job under some other meaning.

use std::collections::BTreeMap;

use crate::ExperimentError;

/// `constraints.params` key: the exact pack items to score, comma or
/// whitespace separated (`task-a,task-b`). Order is kept; duplicates are
/// refused. Names an adaptor cannot find in the pack fail the job closed —
/// a topic that names a task is never scored on a smaller set. A one-item
/// list is the **single-task smoke** shape.
pub const PARAM_TASKS: &str = "tasks";
/// `constraints.params` key: pack items never scored, same list shape.
pub const PARAM_TASK_EXCLUDE: &str = "task_exclude";
/// `constraints.params` key: score only the first N selected items
/// (positive integer; `1` is the single-task smoke shape).
pub const PARAM_N_TASKS: &str = "n_tasks";
/// `constraints.params` key: drop items whose known duration is at or over
/// this many seconds (positive integer). Absent = no duration gate.
pub const PARAM_MAX_TASK_DURATION_S: &str = "max_task_duration_s";
/// `constraints.params` key: `"true"` drops items with no duration metadata
/// when a duration gate is in force.
pub const PARAM_EXCLUDE_UNKNOWN_DURATION: &str = "exclude_unknown_duration";
/// `constraints.params` key: default wall clock, in seconds, for one command
/// the miner's harness runs inside the task environment when the harness
/// itself passes no timeout. Absent = the harness API default (none).
pub const PARAM_EXEC_TIMEOUT_S: &str = "exec_timeout_s";
/// `constraints.params` key: what an item counts as when the **miner's
/// harness** raised before the verifier ran — [`AgentExceptionPolicy`].
pub const PARAM_AGENT_EXCEPTION_POLICY: &str = "agent_exception_policy";
/// `constraints.params` key: multiplier on every pack-declared timeout
/// (positive number ≤ [`MAX_TIMEOUT_MULTIPLIER`]).
pub const PARAM_TIMEOUT_MULTIPLIER: &str = "timeout_multiplier";
/// `constraints.params` key: multiplier on the harness (agent) timeout only.
pub const PARAM_AGENT_TIMEOUT_MULTIPLIER: &str = "agent_timeout_multiplier";
/// `constraints.params` key: multiplier on the verifier timeout only.
pub const PARAM_VERIFIER_TIMEOUT_MULTIPLIER: &str = "verifier_timeout_multiplier";
/// `constraints.params` key: multiplier on the environment build timeout only.
pub const PARAM_ENV_BUILD_TIMEOUT_MULTIPLIER: &str = "env_build_timeout_multiplier";
/// `constraints.params` key: items run at once (positive integer).
pub const PARAM_N_CONCURRENT: &str = "n_concurrent";
/// `constraints.params` key: attempts per item (positive integer).
pub const PARAM_N_ATTEMPTS: &str = "n_attempts";

/// Most items one `tasks` / `task_exclude` list may name (a param value is
/// at most 256 chars, so this is never the binding limit in practice).
pub const MAX_TASK_LIST: usize = 64;
/// Largest timeout multiplier a topic may sign.
pub const MAX_TIMEOUT_MULTIPLIER: f64 = 100.0;
/// Longest item name.
pub const MAX_TASK_ID_LEN: usize = 64;

/// What an item counts as when the miner's harness raised an exception
/// during **its own** phase, before the verifier could run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentExceptionPolicy {
    /// No measurement: the run fails closed (503, no row). The default.
    #[default]
    Fail,
    /// The item scores 0 — the miner's harness did not complete it — and the
    /// exception (type, first line) is recorded in evidence. Failures that
    /// are not the harness's (environment build / start, verifier, setup)
    /// stay unmeasured under this policy too.
    Zero,
}

impl AgentExceptionPolicy {
    /// Wire word (`fail` / `zero`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fail => "fail",
            Self::Zero => "zero",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "fail" => Some(Self::Fail),
            "zero" => Some(Self::Zero),
            _ => None,
        }
    }
}

/// The generic knobs of one paid run, as the signed topic set them.
/// `Default` is "the topic said nothing" — no selection, no gate, no
/// multiplier, fail-closed exception policy.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunPolicy {
    /// Exact items to score, in the topic's order (empty = the adaptor's
    /// whole set).
    pub tasks: Vec<String>,
    /// Items never scored.
    pub task_exclude: Vec<String>,
    /// Keep only the first N selected items.
    pub n_tasks: Option<u32>,
    /// Duration ceiling in seconds.
    pub max_task_duration_s: Option<u64>,
    /// Drop items with unknown duration under a gate.
    pub exclude_unknown_duration: bool,
    /// Default per-command wall clock for the miner's harness.
    pub exec_timeout_s: Option<u64>,
    /// What a harness-raised item counts as.
    pub agent_exception_policy: AgentExceptionPolicy,
    /// Global timeout multiplier.
    pub timeout_multiplier: Option<f64>,
    /// Harness timeout multiplier.
    pub agent_timeout_multiplier: Option<f64>,
    /// Verifier timeout multiplier.
    pub verifier_timeout_multiplier: Option<f64>,
    /// Environment build timeout multiplier.
    pub env_build_timeout_multiplier: Option<f64>,
    /// Items run at once.
    pub n_concurrent: Option<u32>,
    /// Attempts per item.
    pub n_attempts: Option<u32>,
}

/// A pack item name: `[A-Za-z0-9][A-Za-z0-9_.-]{0,63}`, never a path.
#[must_use]
pub fn is_task_id(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=MAX_TASK_ID_LEN).contains(&b.len())
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
        && s != "."
        && s != ".."
}

fn bad(key: &str, value: &str, why: &'static str) -> ExperimentError {
    ExperimentError::BadPolicy {
        key: key.to_owned(),
        value: value.to_owned(),
        why,
    }
}

fn present<'a>(params: &'a BTreeMap<String, String>, key: &str) -> Option<&'a str> {
    params.get(key).map(|s| s.trim()).filter(|s| !s.is_empty())
}

/// Comma / whitespace separated item names; ordered, unique, well-formed.
fn task_list(params: &BTreeMap<String, String>, key: &str) -> Result<Vec<String>, ExperimentError> {
    let Some(raw) = present(params, key) else {
        return Ok(Vec::new());
    };
    let mut out: Vec<String> = Vec::new();
    for name in raw.split(|c: char| c == ',' || c.is_whitespace()) {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        if !is_task_id(name) {
            return Err(bad(
                key,
                raw,
                "item names are [A-Za-z0-9][A-Za-z0-9_.-]{0,63}, separated by commas or spaces",
            ));
        }
        if out.iter().any(|seen| seen == name) {
            return Err(bad(key, raw, "an item is named twice"));
        }
        out.push(name.to_owned());
    }
    if out.is_empty() {
        return Err(bad(key, raw, "the list names no item"));
    }
    if out.len() > MAX_TASK_LIST {
        return Err(bad(key, raw, "too many items in one list"));
    }
    Ok(out)
}

fn positive_u32(
    params: &BTreeMap<String, String>,
    key: &str,
) -> Result<Option<u32>, ExperimentError> {
    match present(params, key) {
        None => Ok(None),
        Some(raw) => raw
            .parse::<u32>()
            .ok()
            .filter(|n| *n > 0)
            .map(Some)
            .ok_or_else(|| bad(key, raw, "must be a positive integer")),
    }
}

fn positive_u64(
    params: &BTreeMap<String, String>,
    key: &str,
) -> Result<Option<u64>, ExperimentError> {
    match present(params, key) {
        None => Ok(None),
        Some(raw) => raw
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .map(Some)
            .ok_or_else(|| bad(key, raw, "must be a positive integer of seconds")),
    }
}

fn multiplier(
    params: &BTreeMap<String, String>,
    key: &str,
) -> Result<Option<f64>, ExperimentError> {
    match present(params, key) {
        None => Ok(None),
        Some(raw) => raw
            .parse::<f64>()
            .ok()
            .filter(|m| m.is_finite() && *m > 0.0 && *m <= MAX_TIMEOUT_MULTIPLIER)
            .map(Some)
            .ok_or_else(|| bad(key, raw, "must be a positive number no larger than 100")),
    }
}

fn bool_word(params: &BTreeMap<String, String>, key: &str) -> Result<bool, ExperimentError> {
    match present(params, key).map(str::to_ascii_lowercase) {
        None => Ok(false),
        Some(w) if w == "true" => Ok(true),
        Some(w) if w == "false" => Ok(false),
        Some(_) => Err(bad(
            key,
            params.get(key).map_or("", String::as_str),
            "must be \"true\" or \"false\"",
        )),
    }
}

impl RunPolicy {
    /// Read the policy from a topic's `constraints.params`. Keys this module
    /// does not know are left alone (they are the adaptor's, as
    /// `PROOF_PARAM_*`); a known key with a malformed value is refused.
    ///
    /// # Errors
    ///
    /// [`ExperimentError::BadPolicy`] naming the first bad knob.
    pub fn from_params(params: &BTreeMap<String, String>) -> Result<Self, ExperimentError> {
        let policy = Self {
            tasks: task_list(params, PARAM_TASKS)?,
            task_exclude: task_list(params, PARAM_TASK_EXCLUDE)?,
            n_tasks: positive_u32(params, PARAM_N_TASKS)?,
            max_task_duration_s: positive_u64(params, PARAM_MAX_TASK_DURATION_S)?,
            exclude_unknown_duration: bool_word(params, PARAM_EXCLUDE_UNKNOWN_DURATION)?,
            exec_timeout_s: positive_u64(params, PARAM_EXEC_TIMEOUT_S)?,
            agent_exception_policy: match present(params, PARAM_AGENT_EXCEPTION_POLICY) {
                None => AgentExceptionPolicy::Fail,
                Some(raw) => AgentExceptionPolicy::parse(raw).ok_or_else(|| {
                    bad(
                        PARAM_AGENT_EXCEPTION_POLICY,
                        raw,
                        "must be \"fail\" (default) or \"zero\"",
                    )
                })?,
            },
            timeout_multiplier: multiplier(params, PARAM_TIMEOUT_MULTIPLIER)?,
            agent_timeout_multiplier: multiplier(params, PARAM_AGENT_TIMEOUT_MULTIPLIER)?,
            verifier_timeout_multiplier: multiplier(params, PARAM_VERIFIER_TIMEOUT_MULTIPLIER)?,
            env_build_timeout_multiplier: multiplier(params, PARAM_ENV_BUILD_TIMEOUT_MULTIPLIER)?,
            n_concurrent: positive_u32(params, PARAM_N_CONCURRENT)?,
            n_attempts: positive_u32(params, PARAM_N_ATTEMPTS)?,
        };
        if let Some(both) = policy
            .tasks
            .iter()
            .find(|t| policy.task_exclude.contains(t))
        {
            return Err(bad(
                PARAM_TASK_EXCLUDE,
                both,
                "an item cannot be both selected and excluded",
            ));
        }
        Ok(policy)
    }

    /// Whether the topic scores exactly one item — the single-task smoke
    /// shape (`tasks` with one name, or `n_tasks = 1`).
    #[must_use]
    pub fn selects_single_item(&self) -> bool {
        self.tasks.len() == 1 || self.n_tasks == Some(1)
    }

    /// One line for a log: what the topic asked for (never a secret).
    #[must_use]
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.tasks.is_empty() {
            parts.push(format!("{}={}", PARAM_TASKS, self.tasks.join(",")));
        }
        if !self.task_exclude.is_empty() {
            parts.push(format!(
                "{}={}",
                PARAM_TASK_EXCLUDE,
                self.task_exclude.join(",")
            ));
        }
        if let Some(n) = self.n_tasks {
            parts.push(format!("{PARAM_N_TASKS}={n}"));
        }
        if let Some(s) = self.max_task_duration_s {
            parts.push(format!("{PARAM_MAX_TASK_DURATION_S}={s}"));
        }
        if let Some(s) = self.exec_timeout_s {
            parts.push(format!("{PARAM_EXEC_TIMEOUT_S}={s}"));
        }
        parts.push(format!(
            "{PARAM_AGENT_EXCEPTION_POLICY}={}",
            self.agent_exception_policy.as_str()
        ));
        parts.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    /// A topic that sets nothing gets the fail-closed default; every knob
    /// is read from the signed params, and unknown keys are left alone.
    #[test]
    fn silence_is_the_fail_closed_default_and_knobs_are_topic_data() {
        let silent = RunPolicy::from_params(&params(&[("tasks_dir", "tasks")])).expect("ok");
        assert_eq!(silent, RunPolicy::default());
        assert_eq!(silent.agent_exception_policy, AgentExceptionPolicy::Fail);
        assert!(!silent.selects_single_item());
        assert_eq!(silent.summary(), "agent_exception_policy=fail");

        let full = RunPolicy::from_params(&params(&[
            (PARAM_TASKS, "task-b, task-a task_c.v2"),
            (PARAM_TASK_EXCLUDE, "task-z"),
            (PARAM_N_TASKS, "2"),
            (PARAM_MAX_TASK_DURATION_S, "3600"),
            (PARAM_EXCLUDE_UNKNOWN_DURATION, "TRUE"),
            (PARAM_EXEC_TIMEOUT_S, "900"),
            (PARAM_AGENT_EXCEPTION_POLICY, " Zero "),
            (PARAM_TIMEOUT_MULTIPLIER, "1.5"),
            (PARAM_AGENT_TIMEOUT_MULTIPLIER, "2"),
            (PARAM_VERIFIER_TIMEOUT_MULTIPLIER, "0.5"),
            (PARAM_ENV_BUILD_TIMEOUT_MULTIPLIER, "3"),
            (PARAM_N_CONCURRENT, "4"),
            (PARAM_N_ATTEMPTS, "1"),
            ("harness_agent", "whatever-the-adaptor-reads"),
        ]))
        .expect("well-formed");
        assert_eq!(
            full.tasks,
            vec!["task-b", "task-a", "task_c.v2"],
            "order kept"
        );
        assert_eq!(full.task_exclude, vec!["task-z"]);
        assert_eq!(full.n_tasks, Some(2));
        assert_eq!(full.max_task_duration_s, Some(3600));
        assert!(full.exclude_unknown_duration);
        assert_eq!(full.exec_timeout_s, Some(900));
        assert_eq!(full.agent_exception_policy, AgentExceptionPolicy::Zero);
        assert_eq!(full.timeout_multiplier, Some(1.5));
        assert_eq!(full.agent_timeout_multiplier, Some(2.0));
        assert_eq!(full.verifier_timeout_multiplier, Some(0.5));
        assert_eq!(full.env_build_timeout_multiplier, Some(3.0));
        assert_eq!((full.n_concurrent, full.n_attempts), (Some(4), Some(1)));
        assert!(!full.selects_single_item(), "three named, two kept");
        let s = full.summary();
        assert!(s.contains("tasks=task-b,task-a,task_c.v2"), "{s}");
        assert!(s.contains("agent_exception_policy=zero"), "{s}");
    }

    /// The single-task smoke is a topic shape, not a code path: one name in
    /// `tasks`, or `n_tasks = 1`.
    #[test]
    fn a_single_item_selection_is_the_smoke_shape() {
        let one = RunPolicy::from_params(&params(&[(PARAM_TASKS, "only-this")])).expect("ok");
        assert!(one.selects_single_item());
        let first = RunPolicy::from_params(&params(&[(PARAM_N_TASKS, "1")])).expect("ok");
        assert!(first.selects_single_item());
        let two = RunPolicy::from_params(&params(&[(PARAM_TASKS, "a,b")])).expect("ok");
        assert!(!two.selects_single_item());
    }

    /// Every malformed value is refused by name — never read as "unset",
    /// never rounded to a nearby meaning.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn malformed_knobs_are_refused_by_name() {
        let refuse = |key: &str, value: &str| {
            let err = RunPolicy::from_params(&params(&[(key, value)]))
                .expect_err(&format!("{key}={value:?} must be refused"));
            match &err {
                ExperimentError::BadPolicy { key: k, .. } => assert_eq!(k, key, "{err}"),
                other => panic!("{key}={value:?}: unexpected {other}"),
            }
            let msg = err.to_string();
            assert!(msg.contains(key), "{msg}");
            msg
        };
        refuse(PARAM_TASKS, "../escape");
        refuse(PARAM_TASKS, "a/b");
        refuse(PARAM_TASKS, "a,a");
        refuse(PARAM_TASKS, ",");
        refuse(PARAM_TASKS, ".");
        refuse(PARAM_TASKS, &"x".repeat(MAX_TASK_ID_LEN + 1));
        refuse(PARAM_TASK_EXCLUDE, "bad name!");
        for key in [
            PARAM_N_TASKS,
            PARAM_N_CONCURRENT,
            PARAM_N_ATTEMPTS,
            PARAM_MAX_TASK_DURATION_S,
            PARAM_EXEC_TIMEOUT_S,
        ] {
            refuse(key, "0");
            refuse(key, "-1");
            refuse(key, "many");
            refuse(key, "1.5");
        }
        for key in [
            PARAM_TIMEOUT_MULTIPLIER,
            PARAM_AGENT_TIMEOUT_MULTIPLIER,
            PARAM_VERIFIER_TIMEOUT_MULTIPLIER,
            PARAM_ENV_BUILD_TIMEOUT_MULTIPLIER,
        ] {
            refuse(key, "0");
            refuse(key, "-2");
            refuse(key, "inf");
            refuse(key, "NaN");
            refuse(key, "101");
            refuse(key, "fast");
        }
        refuse(PARAM_EXCLUDE_UNKNOWN_DURATION, "yes");
        let msg = refuse(PARAM_AGENT_EXCEPTION_POLICY, "zer0");
        assert!(msg.contains("\"fail\" (default) or \"zero\""), "{msg}");
        refuse(PARAM_AGENT_EXCEPTION_POLICY, "skip");
        // Selected and excluded at once is a contradiction, not a precedence.
        let err =
            RunPolicy::from_params(&params(&[(PARAM_TASKS, "a,b"), (PARAM_TASK_EXCLUDE, "b")]))
                .expect_err("contradiction");
        assert!(
            err.to_string().contains("both selected and excluded"),
            "{err}"
        );
    }

    #[test]
    fn task_ids_are_names_not_paths() {
        for good in ["a", "task-1", "Task_2.v3", "0abc"] {
            assert!(is_task_id(good), "{good}");
        }
        for bad in [
            "",
            ".",
            "..",
            "-lead",
            "_lead",
            "a/b",
            "a b",
            "a\\b",
            "ünïcode",
        ] {
            assert!(!is_task_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn param_names_are_stable() {
        assert_eq!(PARAM_TASKS, "tasks");
        assert_eq!(PARAM_TASK_EXCLUDE, "task_exclude");
        assert_eq!(PARAM_N_TASKS, "n_tasks");
        assert_eq!(PARAM_MAX_TASK_DURATION_S, "max_task_duration_s");
        assert_eq!(PARAM_EXCLUDE_UNKNOWN_DURATION, "exclude_unknown_duration");
        assert_eq!(PARAM_EXEC_TIMEOUT_S, "exec_timeout_s");
        assert_eq!(PARAM_AGENT_EXCEPTION_POLICY, "agent_exception_policy");
        assert_eq!(PARAM_TIMEOUT_MULTIPLIER, "timeout_multiplier");
        assert_eq!(PARAM_AGENT_TIMEOUT_MULTIPLIER, "agent_timeout_multiplier");
        assert_eq!(
            PARAM_VERIFIER_TIMEOUT_MULTIPLIER,
            "verifier_timeout_multiplier"
        );
        assert_eq!(
            PARAM_ENV_BUILD_TIMEOUT_MULTIPLIER,
            "env_build_timeout_multiplier"
        );
        assert_eq!(PARAM_N_CONCURRENT, "n_concurrent");
        assert_eq!(PARAM_N_ATTEMPTS, "n_attempts");
        assert_eq!(AgentExceptionPolicy::Fail.as_str(), "fail");
        assert_eq!(AgentExceptionPolicy::Zero.as_str(), "zero");
    }
}
