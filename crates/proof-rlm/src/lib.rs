//! Generic Proof RLM engine — core types. No challenge content.
//!
//! Proof is a **dynamic agentic challenge system**: every research problem is
//! an operator-published signed topic, and each topic's RLM (research
//! lifecycle manager) runs **inside a VM attributed to that topic** where it
//! writes the anti-cheat rules, runs the baseline, inspects and runs miner
//! submissions, and promotes the best artefact. This crate is the control
//! plane's side of that boundary. It knows shapes, not challenges: nothing
//! here names a benchmark, a metric, a model, or a repository.
//!
//! 1. [`RuleSet`] + [`Checklist`] + [`SpendToken`] — the anti-cheat gate as
//!    data. Rules are a versioned vector (v1 = the signed topic's
//!    `checklist`, later versions written by the RLM and persisted by the
//!    store). A checklist is green only when every rule of its version is
//!    ticked with evidence and passes; the token type is the only way to
//!    reach a paid run.
//! 2. [`Lifecycle`] — `draft → owner_presend → awaiting_owner_keys →
//!    provisioning → baselining → open ⇄ evaluating → promoting → closed`,
//!    with `owner_presend` (`askUser`-style) and owner-key presence hooks.
//! 3. [`CustomRunner`] + [`RunnerRegistry`] — `custom_id → runner`, empty by
//!    default. An unregistered id is [`RunnerError::Unregistered`], which the
//!    host turns into a 503 before any row or rent.
//! 4. [`TopicVmOrchestrator`] — create / attach / run / teardown for topic
//!    VMs, with [`VmJob`]s that carry public data only. This crate ships
//!    [`UnwiredVmOrchestrator`] (refuses); the live `FirecrackerOrchestrator`
//!    (crate `proof-vm-fc`) is an HTTPS client of the `proof-vm-orchestrator`
//!    agent on a dedicated KVM host. The generic [`VmBackedRunner`] turns
//!    inspect / evaluate into VM jobs and is only ever registered by an
//!    operator.
//! 5. [`decide_promote`] — pass + green checklist + relative win over the
//!    bar, direction from the topic.
//!
//! Persistence (`proof-rlm-store`), the artefact store, and the
//! `LiveScorer` glue (`proof-rlm-scorer`) live next door. The topic schema
//! (constraints, `eval_executor`, checklist vector, custom id shape) lives
//! in `proof-task`; the live `EvalExecutorOffer` lives in `proof-executor`
//! and a run request records the resolved plan's deadline and commitment.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::must_use_candidate
)]

mod gate;
mod promote;
mod rules;
mod runner;
mod state;
mod vm;

/// Shared test fixtures (fake orchestrator, canned report, placeholder topic).
/// Test builds and the `test-fixtures` feature only; never part of a host binary.
#[cfg(any(test, feature = "test-fixtures"))]
#[path = "fixtures_tests.rs"]
pub mod fixtures;

pub use gate::{authorize_spend, GateError, SpendToken};
pub use promote::{decide_promote, KeepReason, PromoteDecision};
pub use rules::{
    CheckItem, Checklist, ChecklistError, RuleSet, RuleSource, CHECKLIST_SCHEMA, MAX_EVIDENCE_LEN,
};
pub use runner::{
    is_staged_artifact_uri, ArtifactFile, ArtifactTarB64, CustomRunReport, CustomRunRequest,
    CustomRunner, InspectOutcome, JudgeRef, LogFile, ReportError, RunOutcome, RunnerError,
    RunnerRegistry, SandboxPolicy, RUN_REPORT_SCHEMA, RUN_REQUEST_SCHEMA, STAGED_ARTEFACT_SCHEME,
};
pub use state::{
    await_owner_keys, owner_presend, transition, FileKeysProbe, HookError, Lifecycle, NoOwnerHook,
    OwnerDecision, OwnerHook, OwnerKeysProbe, OwnerPrompt, RlmEvent, RlmState, StateError,
    StaticOwnerHook, Transition, OWNER_INFERENCE_KEY_FILE_ENV,
};
pub use vm::{
    run_paid_job, RetainPolicy, TopicVmOrchestrator, TopicVmSpec, UnwiredVmOrchestrator,
    VmBackedRunner, VmError, VmHandle, VmJob, VmJobOutput, VmTemplate, RLM_VM_IMAGE_DIGEST_ENV,
    VM_ORCHESTRATOR_TOKEN_FILE_ENV, VM_ORCHESTRATOR_URL_ENV,
};
// The generic in-guest experiment binding (runner id, pinned pack, VM size
// read from the signed topic's `constraints.params`; operator ceilings).
pub use proof_experiment::{
    ExperimentBinding, ExperimentCeilings, ExperimentError, ExperimentPolicy, ExperimentSpec,
    PackRef, VmShape,
};
// Miner BYOK: the env a topic lets a miner bring to their own paid run.
pub use proof_canon::{
    is_env_name, MinerEnv, MinerEnvError, PARAM_INJECT_MINER_ENV_SISTER, PARAM_MINER_BYOK,
    PARAM_MINER_ENV_ALLOWLIST,
};

#[cfg(test)]
mod tests {
    use super::*;

    /// The crate compiles no challenge: no benchmark, model, repository, or
    /// topic id appears in its non-test source.
    #[test]
    fn no_challenge_content_is_compiled_in() {
        let sources = [
            include_str!("gate.rs"),
            include_str!("promote.rs"),
            include_str!("rules.rs"),
            include_str!("runner.rs"),
            include_str!("state.rs"),
            include_str!("vm.rs"),
            include_str!("lib.rs"),
        ];
        for src in sources {
            let non_test = src.split("#[cfg(test)]").next().unwrap_or("");
            let lower = non_test.to_ascii_lowercase();
            for forbidden in [
                "terminal-bench",
                "terminal bench",
                "tb4",
                "kimi",
                "openrouter",
                "cortexlm/",
                "pass_rate",
                "harness_success",
            ] {
                assert!(!lower.contains(forbidden), "{forbidden:?} is compiled in");
            }
        }
        assert!(RunnerRegistry::new().is_empty());
        assert_eq!(RlmState::ORDER.len(), 9);
    }
}
