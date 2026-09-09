//! Shared test fixtures: a custom-family topic with placeholder ids, a judge
//! offer, a rule set, a run request, a canned report, a green checklist /
//! spend token, a pinned VM template, and a recording fake orchestrator.
//! No network, no secrets, no challenge content — every id here is a
//! placeholder a test invents, never something a runner would recognise.
//! Compiled for tests and the `test-fixtures` feature only (the file name
//! keeps it out of the LOC cap's non-test count, like every other
//! `*_tests.rs`).

// Test-only code: never compiled into a host binary (see the cfg in lib.rs).
#![allow(
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::expect_used,
    clippy::unwrap_used
)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use proof_task::{
    inference_config_commitment, ChecklistRule, InferenceConfig, InferenceMode, InferenceOffer,
    InferenceProvider, InferenceProviderKind, MetricDirection, MetricFamily, OfferStatus, ProofPin,
    TopicDocument, TopicStatus,
};

use crate::gate::{authorize_spend, SpendToken};
use crate::rules::{Checklist, RuleSet};
use crate::runner::{
    ArtifactFile, CustomRunReport, CustomRunRequest, InspectOutcome, LogFile, RunOutcome,
    RUN_REPORT_SCHEMA,
};
use crate::vm::{
    RetainPolicy, TopicVmOrchestrator, TopicVmSpec, VmError, VmHandle, VmJob, VmJobOutput,
    VmTemplate,
};

/// Pin with a topic key and a judge model (no digest unless asked).
pub fn pin() -> ProofPin {
    let mut p = ProofPin {
        eval_image_digest: format!("sha256:{}", "ab".repeat(32)),
        topic_pubkey: "ab".repeat(32),
        ..ProofPin::default()
    };
    p.inference.model = "judge-model-placeholder".into();
    p
}

pub fn offer() -> InferenceOffer {
    let config = InferenceConfig {
        mode: InferenceMode::Chat,
        model_ref: "judge-model-placeholder".into(),
        max_input_tokens: 32_768,
        max_output_tokens: 8_192,
        temperature: Some(0.0),
        top_p: None,
        timeout_ms: None,
    };
    InferenceOffer {
        offer_id: "judge-offer-placeholder".into(),
        provider: InferenceProvider {
            kind: InferenceProviderKind::OpenaiCompatible,
            base_url: "http://127.0.0.1:8000/v1".into(),
        },
        config_commitment: inference_config_commitment(&config, "http://127.0.0.1:8000/v1"),
        config,
        status: OfferStatus::Open,
    }
}

/// A sealed, open custom-family topic with placeholder bindings.
pub fn topic() -> TopicDocument {
    let mut doc = TopicDocument {
        id: "topic-a".into(),
        statement: "Placeholder research problem scored by a topic-minted custom metric.".into(),
        ..TopicDocument::default()
    };
    doc.metric.family = MetricFamily::Custom;
    doc.metric.custom_id = "placeholder_metric".into();
    doc.metric.primary = "primary_value".into();
    doc.metric.direction = MetricDirection::Max;
    doc.metric.epsilon_rel = 0.02;
    doc.constraints.firecracker_required = true;
    doc.constraints.model_pin = Some("vendor/model-placeholder".into());
    doc.constraints.task_slice = Some("slice-placeholder".into());
    doc.constraints
        .params
        .insert("param_a".into(), "value-a".into());
    doc.checklist = vec![
        ChecklistRule {
            id: "rule_a".into(),
            text: "placeholder rule a".into(),
        },
        ChecklistRule {
            id: "rule_b".into(),
            text: "placeholder rule b".into(),
        },
        ChecklistRule {
            id: "rule_c".into(),
            text: "placeholder rule c".into(),
        },
    ];
    doc.baseline.optimizer = "reference-placeholder".into();
    doc.baseline.lr = 1.0;
    doc.baseline.schedule = "n/a".into();
    doc.baseline.dtype = "n/a".into();
    doc.baseline.script_sha256 = "11".repeat(32);
    doc.baseline.metrics_commitment = "22".repeat(32);
    doc.status = TopicStatus::Open;
    doc
}

pub fn rules() -> RuleSet {
    RuleSet::from_topic(&topic()).expect("rules")
}

pub fn request() -> CustomRunRequest {
    CustomRunRequest::from_topic(
        &topic(),
        &pin(),
        &offer(),
        &rules(),
        "digest-a",
        &"ab".repeat(32),
        Some("https://example.invalid/artifact.zip"),
        1,
        "placeholder claim",
    )
    .expect("request")
}

/// A report that echoes `req` with `primary_value`.
pub fn report_for(req: &CustomRunRequest, primary_value: f64) -> CustomRunReport {
    let mut evidence = BTreeMap::new();
    evidence.insert(
        "rows".into(),
        serde_json::json!([{"index": 0, "passed": true}]),
    );
    CustomRunReport {
        schema_version: RUN_REPORT_SCHEMA,
        topic_id: req.topic_id.clone(),
        custom_id: req.custom_id.clone(),
        submission_digest: req.submission_digest.clone(),
        artifact_digest: req.artifact_digest.clone(),
        rules_version: req.rules_version,
        primary_value,
        claim_holds: true,
        sandboxed: true,
        flops_used: Some(1),
        evidence,
    }
}

pub fn green(rules: &RuleSet, submission_digest: &str) -> Checklist {
    let mut c = Checklist::new(rules, submission_digest, "art");
    for r in &rules.rules {
        c.record(&r.id, true, &r.text);
    }
    c
}

pub fn token_for(req: &CustomRunRequest) -> SpendToken {
    let set = rules();
    let mut c = Checklist::new(&set, &req.submission_digest, &req.artifact_digest);
    for r in &set.rules {
        c.record(&r.id, true, &r.text);
    }
    authorize_spend(&c, &set, &req.topic_id, &req.submission_digest).expect("token")
}

pub fn pinned_template() -> VmTemplate {
    VmTemplate {
        image_digest: format!("sha256:{}", "cc".repeat(32)),
        vcpus: 2,
        mem_mib: 4_096,
    }
}

/// Records jobs and answers with canned documents. Never touches the network.
/// Like the agent, it keeps experiment VMs apart from a topic's RLM VM:
/// `attach` never returns one, and every create / teardown is recorded.
pub struct FakeOrchestrator {
    primary: Mutex<f64>,
    red: Mutex<Option<String>>,
    sandboxed: AtomicBool,
    flops_used: Mutex<Option<u64>>,
    fail_run: AtomicBool,
    created: AtomicUsize,
    vms: Mutex<Vec<VmHandle>>,
    /// `vm_id → spec` of every experiment VM ever created.
    experiments: Mutex<Vec<(String, TopicVmSpec)>>,
    /// Every job with the VM it ran on.
    runs: Mutex<Vec<(String, VmJob)>>,
    teardowns: Mutex<Vec<(VmHandle, RetainPolicy)>>,
    proposed: Mutex<Vec<ChecklistRule>>,
}

impl FakeOrchestrator {
    pub fn new(primary: f64) -> Arc<Self> {
        Arc::new(Self {
            primary: Mutex::new(primary),
            red: Mutex::new(None),
            sandboxed: AtomicBool::new(true),
            flops_used: Mutex::new(Some(1)),
            fail_run: AtomicBool::new(false),
            created: AtomicUsize::new(0),
            vms: Mutex::new(Vec::new()),
            experiments: Mutex::new(Vec::new()),
            runs: Mutex::new(Vec::new()),
            teardowns: Mutex::new(Vec::new()),
            proposed: Mutex::new(vec![ChecklistRule {
                id: "rlm_rule".into(),
                text: "a rule the fake rlm wrote".into(),
            }]),
        })
    }

    /// Every job fails inside the guest (`Backend`) until cleared.
    pub fn set_fail_run(&self, v: bool) {
        self.fail_run.store(v, Ordering::SeqCst);
    }

    /// Specs of the experiment VMs created so far, in order.
    pub fn experiments(&self) -> Vec<TopicVmSpec> {
        self.experiments
            .lock()
            .unwrap()
            .iter()
            .map(|(_, s)| s.clone())
            .collect()
    }

    /// `(vm_id, job)` for every job dispatched, in order.
    pub fn runs(&self) -> Vec<(String, VmJob)> {
        self.runs.lock().unwrap().clone()
    }

    /// Every teardown, in order.
    pub fn teardowns(&self) -> Vec<(VmHandle, RetainPolicy)> {
        self.teardowns.lock().unwrap().clone()
    }

    fn is_experiment(&self, vm_id: &str) -> bool {
        self.experiments
            .lock()
            .unwrap()
            .iter()
            .any(|(id, _)| id == vm_id)
    }

    pub fn set_primary(&self, v: f64) {
        *self.primary.lock().unwrap() = v;
    }

    /// What every report measures as `flops_used` (`None` = the runner
    /// forgot to measure, which the host must refuse).
    pub fn set_flops_used(&self, v: Option<u64>) {
        *self.flops_used.lock().unwrap() = v;
    }

    /// Make inspections fail this rule id (None = green).
    pub fn set_red(&self, id: Option<&str>) {
        *self.red.lock().unwrap() = id.map(str::to_owned);
    }

    pub fn set_sandboxed(&self, v: bool) {
        self.sandboxed.store(v, Ordering::SeqCst);
    }

    pub fn set_proposed(&self, rules: Vec<ChecklistRule>) {
        *self.proposed.lock().unwrap() = rules;
    }

    pub fn created(&self) -> usize {
        self.created.load(Ordering::SeqCst)
    }

    pub fn jobs(&self) -> Vec<VmJob> {
        self.runs
            .lock()
            .unwrap()
            .iter()
            .map(|(_, j)| j.clone())
            .collect()
    }

    /// VMs alive right now (RLM and experiment).
    pub fn vms(&self) -> Vec<VmHandle> {
        self.vms.lock().unwrap().clone()
    }

    fn report(&self, req: &CustomRunRequest) -> CustomRunReport {
        let mut r = report_for(req, *self.primary.lock().unwrap());
        r.sandboxed = self.sandboxed.load(Ordering::SeqCst);
        r.flops_used = *self.flops_used.lock().unwrap();
        r
    }
}

#[async_trait]
impl TopicVmOrchestrator for FakeOrchestrator {
    fn ready(&self) -> Result<(), VmError> {
        Ok(())
    }

    async fn create(&self, spec: &TopicVmSpec) -> Result<VmHandle, VmError> {
        spec.validate()?;
        let n = self.created.fetch_add(1, Ordering::SeqCst);
        let h = VmHandle {
            topic_id: spec.topic_id.clone(),
            vm_id: format!("vm-{n}"),
        };
        if spec.experiment.is_some() {
            self.experiments
                .lock()
                .unwrap()
                .push((h.vm_id.clone(), spec.clone()));
        }
        self.vms.lock().unwrap().push(h.clone());
        Ok(h)
    }

    async fn attach(&self, topic_id: &str) -> Result<Option<VmHandle>, VmError> {
        Ok(self
            .vms
            .lock()
            .unwrap()
            .iter()
            .find(|h| h.topic_id == topic_id && !self.is_experiment(&h.vm_id))
            .cloned())
    }

    async fn run(&self, handle: &VmHandle, job: VmJob) -> Result<VmJobOutput, VmError> {
        assert!(
            self.vms.lock().unwrap().contains(handle),
            "job on an unknown vm"
        );
        self.runs
            .lock()
            .unwrap()
            .push((handle.vm_id.clone(), job.clone()));
        if self.fail_run.load(Ordering::SeqCst) {
            return Err(VmError::Backend("guest: injected run failure".into()));
        }
        Ok(match job {
            VmJob::ProposeRules { .. } => VmJobOutput::Rules(self.proposed.lock().unwrap().clone()),
            VmJob::Baseline { request } => VmJobOutput::Baseline(self.report(&request)),
            VmJob::Inspect { request, rules } => {
                let red = self.red.lock().unwrap().clone();
                let mut checklist =
                    Checklist::new(&rules, &request.submission_digest, &request.artifact_digest);
                for r in &rules.rules {
                    checklist.record(&r.id, red.as_deref() != Some(r.id.as_str()), &r.text);
                }
                VmJobOutput::Inspected(InspectOutcome {
                    checklist,
                    artifact: vec![ArtifactFile {
                        path: "src/main.rs".into(),
                        bytes: b"fn main() {}\n".to_vec(),
                    }],
                })
            }
            VmJob::Evaluate { request, .. } => VmJobOutput::Evaluated(RunOutcome {
                report: self.report(&request),
                logs: vec![LogFile {
                    name: "run.log".into(),
                    bytes: b"ok\n".to_vec(),
                }],
            }),
            VmJob::Archive { .. } => VmJobOutput::Archived,
        })
    }

    async fn teardown(&self, handle: &VmHandle, policy: RetainPolicy) -> Result<bool, VmError> {
        self.vms.lock().unwrap().retain(|h| h != handle);
        self.teardowns
            .lock()
            .unwrap()
            .push((handle.clone(), policy));
        Ok(true)
    }
}

/// `request()` with the in-guest experiment binding a topic would sign:
/// runner id + pinned pack digest (placeholders), optional size ask.
pub fn experiment_request(vcpus: Option<u32>) -> CustomRunRequest {
    let mut req = request();
    let params = &mut req.constraints.params;
    params.insert(
        proof_experiment::PARAM_RUNNER.into(),
        "placeholder_in_guest_runner".into(),
    );
    params.insert(
        proof_experiment::PARAM_PACK_DIGEST.into(),
        format!("sha256:{}", "ee".repeat(32)),
    );
    if let Some(n) = vcpus {
        params.insert(proof_experiment::PARAM_VCPUS.into(), n.to_string());
    }
    req
}
